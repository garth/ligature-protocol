//! A device's screen watched live on another of the person's devices. Spec
//! chapter 05 section 3h; milestone 8 (LP-148).
//!
//! The sharer's platform is behind the [`Capture`] trait: it lists displays
//! and windows and starts a share, which delivers encoded frames, and sound
//! when asked, on a channel until it ends. The viewer's side is a sink the
//! daemon hands to whoever asked to watch (`lpview` over the control socket,
//! the phone through its node): the reassembled frames, the packets of sound
//! and the end of the share, with a handle to ask for a keyframe or to stop.
//! The [`ScreenSync`] negotiates `start`, chunks frames within the message
//! bound, drops rather than queues when the stream holds it back (and asks
//! the encoder for a keyframe once it has dropped), answers `keyframe`, and
//! ends a share on every path: the viewer's `stop`, the sharer's, the
//! platform's withdrawn consent, a failed capture, the stream's end and the
//! connection's.
//!
//! Milestone 9 (LP-199) adds the cast: the sharer opens the stream with an
//! `offer` and the receiver answers `start`, from which point a cast and a
//! watch are the same share. An offer waits for whoever shows pictures on
//! the receiving device (`lpview`, started by the daemon; the app on the
//! phone) to accept or decline it, and sound rides on the share's stream as
//! `audio` messages between the frame chunks, never dropped.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_core::input::Region;
use lp_core::screen::{
    Audio, AudioFormat, CHUNK_BYTES, Display, Displays, Format, Frame, Offer, RefuseReason,
    Refused, SCREEN_TYPE, ScreenMessage, ScreenParams, Source, SourceKind, Start, Started, Stop,
    StopReason, Window, chunk_frame, fit, screen_descriptor,
};
use lp_core::{Error, PublicKey};
use lp_identity::trust::Recognition;
use lp_transport::manager::{StreamHandler, open_on};
use lp_transport::stream::FramedStream;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::daemon::Daemon;

pub mod encode;
pub mod ogg;
pub mod sound;

/// How often the capture's flags are re-read for the manifest.
const ADVERTISE_POLL: Duration = Duration::from_secs(1);
/// How long the viewer waits for `started` or `refused`, which may wait on
/// the sharer's consent dialog.
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How long an offer waits for someone at the receiver to take it before it
/// is refused as declined.
const OFFER_TIMEOUT: Duration = Duration::from_secs(60);
/// The fastest frame rate a sharer agrees to.
const MAX_FRAME_RATE: u32 = 60;
/// Whole frames waiting for the viewer before the oldest are dropped.
const QUEUED_FRAMES: usize = 8;
/// The smallest cap on a reassembled frame, for a tiny picture.
const MIN_FRAME_CAP: usize = 1 << 20;

/// One encoded frame from the platform's encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// When it was captured, microseconds since the share started.
    pub at: u64,
    /// Whether it is a keyframe.
    pub key: bool,
    /// The encoded bytes, an Annex B access unit for H.264.
    pub bytes: Vec<u8>,
}

/// One packet of sound from the platform's encoder: 20 ms of Opus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPacket {
    /// When its first sample was captured, microseconds since the share
    /// started.
    pub at: u64,
    /// The packet.
    pub bytes: Vec<u8>,
}

/// What a running capture delivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// A frame.
    Frame(EncodedFrame),
    /// A packet of sound, when the share's settings asked for it.
    Audio(AudioPacket),
    /// The capture ended on its own: consent withdrawn, or a failure.
    Ended(StopReason),
}

/// What the sync asks of a running capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareControl {
    /// Encode the next frame as a keyframe.
    Keyframe,
    /// Stop capturing.
    Stop,
}

/// The settings a share runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The display's id, or the window's when `kind` says so.
    pub display: String,
    /// Whether `display` names a display or a window.
    pub kind: SourceKind,
    /// The format picked from the viewer's offer (ADR 0044).
    pub format: Format,
    /// The audio format picked, when sound was agreed: the capture then
    /// delivers [`Captured::Audio`] packets beside the frames.
    pub audio: Option<AudioFormat>,
    /// The picture's width.
    pub width: u32,
    /// The picture's height.
    pub height: u32,
    /// The frame rate.
    pub frame_rate: u32,
    /// The viewing device's name, for the platform's sharing indicator.
    pub viewer: String,
    /// Whether this device started the share, as a cast: the indicator says
    /// so.
    pub cast: bool,
}

impl Settings {
    fn started(&self) -> Started {
        let (display, window) = match self.kind {
            SourceKind::Display => (Some(self.display.clone()), None),
            SourceKind::Window => (None, Some(self.display.clone())),
        };
        Started {
            display,
            window,
            format: self.format,
            audio: self.audio,
            width: self.width,
            height: self.height,
            frame_rate: self.frame_rate,
        }
    }
}

/// A running capture: frames in, control out. Dropping it stops the capture.
pub struct Share {
    /// The frames, and the end.
    pub frames: mpsc::UnboundedReceiver<Captured>,
    /// Keyframe requests and the stop.
    pub control: mpsc::UnboundedSender<ShareControl>,
    /// Where the shared display lies in the whole screen, when it is one of
    /// several: the viewer's input positions are mapped through it.
    pub region: Option<Region>,
}

/// The sharer's platform: displays and windows, and a share of one.
pub trait Capture: Send + Sync {
    /// Whether this device shares right now and the picture and audio
    /// formats it encodes; `views`, `receives`, `decodes` and the viewer
    /// bounds are the sync's, not the capture's.
    fn params(&self) -> ScreenParams;
    /// The displays this device can share.
    fn displays(&self) -> Vec<Display>;
    /// The windows this device can share by id, where its platform lets it
    /// pick one without a picker; empty elsewhere.
    fn windows(&self) -> Vec<Window> {
        Vec::new()
    }
    /// Lets the platform's own picker choose a source of `kind` for a cast,
    /// where the platform has one (the ScreenCast portal on Linux), and
    /// returns it with an id a later `start` may name. Blocks on the picker,
    /// so it runs off the async runtime. Elsewhere there is nothing to pick.
    ///
    /// # Errors
    ///
    /// `NoDisplay` when the platform has no picker for `kind`, `Consent`
    /// when the person dismissed it.
    fn pick(&self, _kind: SourceKind) -> Result<Source, RefuseReason> {
        Err(RefuseReason::NoDisplay)
    }
    /// Where `display` lies in the whole screen, for a device with several.
    fn region(&self, _display: &str) -> Option<Region> {
        None
    }
    /// Starts a share with `settings`, which the sync has already fitted to
    /// the source and the viewer. Blocks on the platform's consent where it
    /// has one, so it runs off the async runtime. A window source is fitted
    /// into the picture keeping its aspect when it changes shape, the spare
    /// area black.
    ///
    /// # Errors
    ///
    /// Returns why the share does not start.
    fn start(&self, settings: &Settings) -> Result<Share, RefuseReason>;
}

/// What a viewer gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Viewed {
    /// A whole frame, its chunks joined.
    Frame {
        /// The frame's number.
        seq: u64,
        /// When it was captured, microseconds since the share started.
        at: u64,
        /// Whether it is a keyframe.
        key: bool,
        /// The encoded bytes.
        bytes: Vec<u8>,
    },
    /// A packet of sound, in the agreed audio format.
    Audio {
        /// The packet's number.
        seq: u64,
        /// When its first sample was captured, on the frames' clock.
        at: u64,
        /// The packet.
        bytes: Vec<u8>,
    },
    /// The share ended.
    Stopped(StopReason),
}

/// A share this device watches: what `watch` returns.
pub struct Watching {
    /// What the sharer settled on.
    pub started: Started,
    /// The frames and the end. Bounded: a viewer that falls behind loses
    /// frames up to the next keyframe rather than queueing them (LP-186).
    pub frames: mpsc::Receiver<Viewed>,
    /// Keyframe requests and the stop.
    pub control: mpsc::UnboundedSender<ShareControl>,
}

impl std::fmt::Debug for Watching {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Watching({:?})", self.started)
    }
}

/// A cast this device runs: what `cast` returns once the receiver took it.
#[derive(Debug)]
pub struct Casting {
    /// What was settled on.
    pub started: Started,
    /// Why the cast ended, when it does.
    pub ended: oneshot::Receiver<StopReason>,
}

/// What a device casts: a display (the primary one when neither is named)
/// or a window of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CastSource {
    /// A display's id.
    pub display: Option<String>,
    /// A window's id, instead.
    pub window: Option<String>,
}

/// What is told when a cast is offered to this device: the caster and its
/// offer. The daemon sets one so something shows the picture (`lpview`, the
/// phone's app), which then accepts or declines through the sync.
pub type OnOffer = Arc<dyn Fn(PublicKey, Offer) + Send + Sync>;

/// An offer this device holds, for `lpctl screen status` and the receivers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Offered {
    /// The caster.
    pub peer: PublicKey,
    /// What it offers.
    pub offer: Offer,
}

impl std::fmt::Debug for Share {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Share")
    }
}

/// A share this device runs, for `lpctl screen status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sharing {
    /// The viewer.
    pub peer: PublicKey,
    /// What it runs with.
    pub started: Started,
    /// Frames sent.
    pub frames: u64,
    /// Frames dropped because the stream held them back.
    pub dropped: u64,
    /// Packets of sound sent.
    #[serde(default)]
    pub audio: u64,
    /// Whether this device started it, as a cast.
    #[serde(default)]
    pub cast: bool,
}

/// A share this device watches, for `lpctl screen status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchingStatus {
    /// The sharer.
    pub peer: PublicKey,
    /// What it runs with.
    pub started: Started,
    /// Frames received whole.
    pub frames: u64,
    /// Packets of sound received.
    #[serde(default)]
    pub audio: u64,
    /// Whether the sharer started it, as a cast this device received.
    #[serde(default)]
    pub cast: bool,
}

/// What `lpctl screen status` shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenStatus {
    /// What this device advertises.
    pub shares: bool,
    /// What this device advertises.
    pub views: bool,
    /// What this device advertises: it shows casts offered to it.
    #[serde(default)]
    pub receives: bool,
    /// The shares this device runs.
    pub sharing: Vec<Sharing>,
    /// The shares this device watches.
    pub watching: Vec<WatchingStatus>,
    /// The casts offered to this device that nobody has taken yet.
    #[serde(default)]
    pub offered: Vec<Offered>,
}

struct SharingState {
    peer: PublicKey,
    started: Started,
    frames: u64,
    dropped: u64,
    audio: u64,
    cast: bool,
    /// Ends the stream task from outside: a stop from the sharer's side.
    stop: mpsc::UnboundedSender<StopReason>,
}

struct WatchingState {
    peer: PublicKey,
    started: Started,
    frames: u64,
    audio: u64,
    cast: bool,
    /// A stop from outside the viewer: `lpctl screen stop`.
    stop: mpsc::UnboundedSender<ShareControl>,
}

/// How whoever shows pictures on this device answered an offer.
enum Decision {
    /// Take it, with sound if `audio` and the offer has any; the share's
    /// handle comes back on the channel.
    Accept {
        audio: bool,
        reply: oneshot::Sender<Result<Watching, Error>>,
    },
    /// Leave it.
    Decline,
}

struct PendingOffer {
    offer: Offer,
    decide: Option<oneshot::Sender<Decision>>,
}

/// The screen synchroniser for one daemon.
pub struct ScreenSync {
    daemon: Arc<Daemon>,
    capture: Option<Arc<dyn Capture>>,
    /// What this device takes as a viewer, if it views at all.
    viewer: Option<ScreenParams>,
    /// What shows an offered cast on this device, if anything: set, the
    /// device advertises `receives`.
    on_offer: Mutex<Option<OnOffer>>,
    /// Shares running, by display or window id.
    sharing: Mutex<HashMap<String, SharingState>>,
    /// Shares watched, by sharer and source id.
    watching: Mutex<HashMap<(PublicKey, String), WatchingState>>,
    /// Offers waiting for someone here to take them, by caster.
    pending: Mutex<HashMap<PublicKey, PendingOffer>>,
    offered: tokio::sync::Mutex<Option<ScreenParams>>,
}

impl std::fmt::Debug for ScreenSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScreenSync")
    }
}

impl ScreenSync {
    /// Starts the screen family for `daemon`: a capture if the platform
    /// shares, viewer params if it views (what its decoder takes).
    #[must_use]
    pub fn start(
        daemon: Arc<Daemon>,
        capture: Option<Arc<dyn Capture>>,
        viewer: Option<ScreenParams>,
    ) -> Arc<Self> {
        let this = Arc::new(Self {
            daemon: daemon.clone(),
            capture,
            viewer,
            on_offer: Mutex::new(None),
            sharing: Mutex::new(HashMap::new()),
            watching: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            offered: tokio::sync::Mutex::new(None),
        });
        daemon.manager().on_stream(SCREEN_TYPE, this.handler());
        let advertise = this.clone();
        tokio::spawn(async move {
            loop {
                advertise.advertise().await;
                tokio::time::sleep(ADVERTISE_POLL).await;
            }
        });
        this
    }

    /// Sets what shows an offered cast on this device, after which the
    /// device advertises `receives` (within the manifest's poll). On a
    /// desktop the daemon starts `lpview --receive`; on the phone the node
    /// tells the app.
    pub fn set_on_offer(&self, on_offer: OnOffer) {
        *lock(&self.on_offer) = Some(on_offer);
    }

    /// What this device advertises right now.
    #[must_use]
    pub fn params(&self) -> ScreenParams {
        let mut params = self.viewer.clone().unwrap_or_default();
        params.views = self.viewer.is_some();
        params.receives = params.views && lock(&self.on_offer).is_some();
        match self.capture.as_ref().map(|c| c.params()) {
            Some(capture) => {
                params.shares = capture.shares;
                params.encodes = capture.encodes;
                params.audio_encodes = capture.audio_encodes;
            }
            None => {
                params.shares = false;
                params.encodes = Vec::new();
                params.audio_encodes = Vec::new();
            }
        }
        params
    }

    /// Re-reads the capture and tells peers if the descriptor changed.
    pub async fn advertise(&self) {
        let mut offered = self.offered.lock().await;
        let params = self.params();
        if offered.as_ref() == Some(&params) {
            return;
        }
        self.daemon.registry().withdraw(SCREEN_TYPE).await;
        if params.offers_anything() {
            self.daemon
                .registry()
                .offer(screen_descriptor(&params))
                .await;
        }
        tracing::info!(
            shares = params.shares,
            views = params.views,
            receives = params.receives,
            "screen capability advertised"
        );
        *offered = Some(params);
    }

    /// What `lpctl screen status` shows.
    #[must_use]
    pub fn status(&self) -> ScreenStatus {
        let params = self.params();
        ScreenStatus {
            shares: params.shares,
            views: params.views,
            receives: params.receives,
            sharing: lock(&self.sharing)
                .values()
                .map(|s| Sharing {
                    peer: s.peer,
                    started: s.started.clone(),
                    frames: s.frames,
                    dropped: s.dropped,
                    audio: s.audio,
                    cast: s.cast,
                })
                .collect(),
            watching: lock(&self.watching)
                .values()
                .map(|w| WatchingStatus {
                    peer: w.peer,
                    started: w.started.clone(),
                    frames: w.frames,
                    audio: w.audio,
                    cast: w.cast,
                })
                .collect(),
            offered: lock(&self.pending)
                .iter()
                .map(|(peer, p)| Offered {
                    peer: *peer,
                    offer: p.offer.clone(),
                })
                .collect(),
        }
    }

    /// The casts offered to this device that nobody has taken yet.
    #[must_use]
    pub fn offers(&self) -> Vec<Offered> {
        self.status().offered
    }

    /// Asks `peer` what displays it has.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] when `peer` is not this person's device or
    /// does not share, and [`Error::Transport`] when the stream fails.
    pub async fn displays(&self, peer: &PublicKey) -> Result<Vec<Display>, Error> {
        Ok(self.sources(peer).await?.displays.unwrap_or_default())
    }

    /// This device's own displays and the windows it can share by id: what
    /// a cast from here may name. Empty lists where it shares nothing.
    #[must_use]
    pub fn local_sources(&self) -> Displays {
        match &self.capture {
            Some(capture) => Displays {
                displays: Some(capture.displays()),
                windows: Some(capture.windows()),
            },
            None => Displays {
                displays: Some(Vec::new()),
                windows: Some(Vec::new()),
            },
        }
    }

    /// Asks `peer` what displays and windows it has.
    ///
    /// # Errors
    ///
    /// As [`ScreenSync::displays`].
    pub async fn sources(&self, peer: &PublicKey) -> Result<Displays, Error> {
        self.sharer_params(peer)?;
        let mut stream = self.open(peer).await?;
        stream
            .send(&ScreenMessage::Displays(Displays::default()))
            .await?;
        let answer = tokio::time::timeout(Duration::from_secs(10), stream.recv::<ScreenMessage>())
            .await
            .map_err(|_| Error::Transport(format!("{peer} did not list its displays")))??;
        stream.finish();
        match answer {
            Some(ScreenMessage::Displays(
                listed @ Displays {
                    displays: Some(_), ..
                },
            )) => Ok(listed),
            other => Err(Error::Transport(format!(
                "{peer} answered displays with {other:?}"
            ))),
        }
    }

    /// Starts watching a display of `peer`: the primary one when `display`
    /// is `None`, no larger or faster than this device's viewer params and
    /// than `start` asks.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] when `peer` is not this person's device, does
    /// not share, or refused (with the reason), [`Error::Io`] when this
    /// device does not view, and [`Error::Transport`] when the stream fails.
    pub async fn watch(
        self: &Arc<Self>,
        peer: &PublicKey,
        start: Start,
    ) -> Result<Watching, Error> {
        let Some(viewer) = self.viewer.clone() else {
            return Err(Error::Io("this device does not view".to_owned()));
        };
        let sharer = self.sharer_params(peer)?;
        // The offer: what this device decodes, in its order, or the part of
        // it the caller narrowed to (ADR 0044).
        let formats: Vec<Format> = if start.formats.is_empty() {
            viewer.decodes.clone()
        } else {
            start
                .formats
                .iter()
                .copied()
                .filter(|f| viewer.decodes.contains(f))
                .collect()
        };
        if !formats.iter().any(|f| sharer.encodes.contains(f)) {
            return Err(Error::Chain(
                "no picture format that this device decodes and the other encodes".to_owned(),
            ));
        }
        // Sound, when asked for: what this device decodes, in its order, or
        // the part the caller narrowed to, and only what the sharer encodes;
        // a share whose sound cannot be agreed is silent rather than refused.
        let audio: Vec<AudioFormat> = start
            .audio
            .iter()
            .copied()
            .filter(|f| viewer.audio_decodes.contains(f) && sharer.audio_encodes.contains(f))
            .collect();
        if !start.audio.is_empty() && audio.is_empty() {
            tracing::debug!(%peer, "no audio format that this device decodes and the other encodes; a silent share");
        }
        let start = Start {
            formats,
            audio,
            width: start.width.min(viewer.max_width),
            height: start.height.min(viewer.max_height),
            frame_rate: start.frame_rate.min(viewer.max_frame_rate),
            ..start
        };
        let mut stream = self.open(peer).await?;
        stream.send(&ScreenMessage::Start(start.clone())).await?;
        let started = self.await_started(peer, &mut stream, &start, None).await?;
        Ok(self.watching(peer, started, stream, false))
    }

    /// Waits for the sharer's answer to `start` and checks it against what
    /// was asked (LP-186), or against the source `offered` when `start`
    /// answered an offer.
    async fn await_started(
        &self,
        peer: &PublicKey,
        stream: &mut FramedStream,
        start: &Start,
        offered: Option<&Source>,
    ) -> Result<Started, Error> {
        let answer = tokio::time::timeout(START_TIMEOUT, stream.recv::<ScreenMessage>())
            .await
            .map_err(|_| Error::Transport(format!("{peer} did not answer start")))??;
        let started = match answer {
            Some(ScreenMessage::Started(started)) => started,
            Some(ScreenMessage::Refused(Refused { reason })) => {
                let name = self
                    .daemon
                    .device_display_name(peer)
                    .unwrap_or_else(|| peer.fingerprint());
                return Err(Error::Refused(format!(
                    "{name} refused the share: {reason}"
                )));
            }
            other => {
                stream.reset(lp_core::frame::MALFORMED_CODE);
                return Err(Error::Transport(format!(
                    "{peer} answered start with {other:?}"
                )));
            }
        };
        if let Err(why) = check_started(start, &started, offered) {
            stream.reset(lp_core::frame::MALFORMED_CODE);
            return Err(Error::Transport(format!(
                "{peer} answered start with what was not asked for: {why}"
            )));
        }
        Ok(started)
    }

    /// Records a share this device watches and starts its stream task.
    fn watching(
        self: &Arc<Self>,
        peer: &PublicKey,
        started: Started,
        stream: FramedStream,
        cast: bool,
    ) -> Watching {
        let (frames_tx, frames) = mpsc::channel(QUEUED_FRAMES);
        let (control, control_rx) = mpsc::unbounded_channel();
        let key = (*peer, started.source_id().to_owned());
        lock(&self.watching).insert(
            key.clone(),
            WatchingState {
                peer: *peer,
                started: started.clone(),
                frames: 0,
                audio: 0,
                cast,
                stop: control.clone(),
            },
        );
        tracing::info!(%peer, source = %started.source_id(), kind = %started.kind(), width = started.width, height = started.height, audio = ?started.audio, cast, "watching");
        let this = self.clone();
        tokio::spawn(async move { this.run_watch(key, stream, frames_tx, control_rx).await });
        Watching {
            started,
            frames,
            control,
        }
    }

    /// Casts a display or a window of this device to `peer`, which must say
    /// it receives: opens the stream with an `offer` and waits for the
    /// receiver to take it. With `audio`, the device's sound goes too when
    /// both sides have a format for it. Returns once the share runs, with a
    /// channel that says why it ended.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] when `peer` is not this person's device or
    /// does not receive, [`Error::Io`] when this device does not share or has
    /// no such source, [`Error::Refused`] when the receiver declined (with
    /// the reason), and [`Error::Transport`] when the stream fails.
    pub async fn cast(
        self: &Arc<Self>,
        peer: &PublicKey,
        source: CastSource,
        audio: bool,
    ) -> Result<Casting, Error> {
        let Some(capture) = self.capture.clone() else {
            return Err(Error::Io("this device does not share".to_owned()));
        };
        let receiver = self.receiver_params(peer)?;
        let params = capture.params();
        if !params.shares {
            return Err(Error::Io(
                "this device does not share its screen right now".to_owned(),
            ));
        }
        let (chosen, kind) = match (&source.window, &source.display) {
            // An empty window id: the platform's picker chooses.
            (Some(id), _) if id.is_empty() => {
                let picker = capture.clone();
                let picked = tokio::task::spawn_blocking(move || picker.pick(SourceKind::Window))
                    .await
                    .map_err(|_| Error::Io("the window picker did not answer".to_owned()))?
                    .map_err(|reason| {
                        Error::Refused(format!("no window was picked to cast: {reason}"))
                    })?;
                (picked, SourceKind::Window)
            }
            (Some(id), _) => (
                capture
                    .windows()
                    .iter()
                    .find(|w| &w.id == id)
                    .map(Source::window)
                    .ok_or_else(|| Error::Io(format!("no window {id}")))?,
                SourceKind::Window,
            ),
            (None, Some(id)) => (
                capture
                    .displays()
                    .iter()
                    .find(|d| &d.id == id)
                    .map(Source::display)
                    .ok_or_else(|| Error::Io(format!("no display {id}")))?,
                SourceKind::Display,
            ),
            (None, None) => (
                primary(&capture.displays())
                    .map(Source::display)
                    .ok_or_else(|| Error::Io("no display to cast".to_owned()))?,
                SourceKind::Display,
            ),
        };
        if lock(&self.sharing).contains_key(&chosen.id) {
            return Err(Error::Refused(format!("{} is already shared", chosen.id)));
        }
        let offer = Offer {
            source: chosen.clone(),
            formats: params.encodes.clone(),
            max_frame_rate: MAX_FRAME_RATE,
            audio: if audio {
                params.audio_encodes.clone()
            } else {
                Vec::new()
            },
        };
        if !offer.formats.iter().any(|f| receiver.decodes.contains(f)) {
            return Err(Error::Chain(
                "no picture format that this device encodes and the other decodes".to_owned(),
            ));
        }
        let name = self
            .daemon
            .device_display_name(peer)
            .unwrap_or_else(|| peer.fingerprint());
        let mut stream = self.open(peer).await?;
        stream.send(&ScreenMessage::Offer(offer)).await?;
        let answer = tokio::time::timeout(
            OFFER_TIMEOUT + START_TIMEOUT,
            stream.recv::<ScreenMessage>(),
        )
        .await
        .map_err(|_| Error::Transport(format!("{name} did not answer the offer")))??;
        let start = match answer {
            Some(ScreenMessage::Start(start)) => start,
            Some(ScreenMessage::Refused(Refused { reason })) => {
                return Err(Error::Refused(format!(
                    "{name} did not take the cast: {reason}"
                )));
            }
            other => {
                stream.reset(lp_core::frame::MALFORMED_CODE);
                return Err(Error::Transport(format!(
                    "{peer} answered the offer with {other:?}"
                )));
            }
        };
        let (settings, share) = match self
            .start_share(&capture, peer, &chosen, kind, &start, true)
            .await
        {
            Ok(started) => started,
            Err(reason) => {
                refuse(&mut stream, reason).await;
                return Err(Error::Refused(format!(
                    "the cast to {name} could not start: {reason}"
                )));
            }
        };
        let started = settings.started();
        let (ended_tx, ended) = oneshot::channel();
        let this = self.clone();
        let peer = *peer;
        tokio::spawn(async move {
            let reason = this.run_started(&peer, settings, share, stream, true).await;
            let _ = ended_tx.send(reason);
        });
        Ok(Casting { started, ended })
    }

    /// Takes the cast `peer` offered: `start` goes back with what this
    /// device decodes, and the share's handle is returned once it runs.
    /// With `audio`, sound is asked for where the offer has any.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] when no offer from `peer` is waiting,
    /// [`Error::Refused`] when the caster refused after all, and
    /// [`Error::Transport`] when the stream fails.
    pub async fn accept(&self, peer: &PublicKey, audio: bool) -> Result<Watching, Error> {
        let (reply_tx, reply) = oneshot::channel();
        self.decide(
            peer,
            Decision::Accept {
                audio,
                reply: reply_tx,
            },
        )?;
        reply
            .await
            .map_err(|_| Error::Transport(format!("the offer from {peer} went away")))?
    }

    /// Declines the cast `peer` offered.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] when no offer from `peer` is waiting.
    pub fn decline(&self, peer: &PublicKey) -> Result<(), Error> {
        self.decide(peer, Decision::Decline)
    }

    fn decide(&self, peer: &PublicKey, decision: Decision) -> Result<(), Error> {
        let decide = lock(&self.pending)
            .get_mut(peer)
            .and_then(|p| p.decide.take())
            .ok_or_else(|| Error::Chain(format!("no cast offered by {peer} is waiting")))?;
        decide
            .send(decision)
            .map_err(|_| Error::Chain(format!("the offer from {peer} is no longer waiting")))
    }

    /// Stops every share this device runs for `peer` and every share it
    /// watches from `peer`, with `reason`. Returns how many ended.
    pub fn stop_with(&self, peer: &PublicKey, reason: StopReason) -> usize {
        let mut n = 0;
        for s in lock(&self.sharing).values().filter(|s| s.peer == *peer) {
            let _ = s.stop.send(reason);
            n += 1;
        }
        for w in lock(&self.watching).values().filter(|w| w.peer == *peer) {
            let _ = w.stop.send(ShareControl::Stop);
            n += 1;
        }
        n
    }

    /// Stops every share this device runs, as its user asked.
    pub fn stop_sharing(&self) -> usize {
        let mut n = 0;
        for s in lock(&self.sharing).values() {
            let _ = s.stop.send(StopReason::Sharer);
            n += 1;
        }
        n
    }

    /// A peer disconnected: its shares, both ways, are over.
    pub fn detach(&self, peer: &PublicKey) {
        let gone = self.stop_with(peer, StopReason::Error);
        if gone > 0 {
            tracing::info!(%peer, gone, "shares ended with the connection");
        }
    }

    async fn open(&self, peer: &PublicKey) -> Result<FramedStream, Error> {
        let conn = self
            .daemon
            .manager()
            .peer(peer)
            .ok_or_else(|| Error::Transport(format!("{peer} is not connected")))?;
        open_on(&conn, SCREEN_TYPE).await
    }

    fn sharer_params(&self, peer: &PublicKey) -> Result<ScreenParams, Error> {
        let params = self.peer_params(peer)?;
        if !params.shares {
            return Err(Error::Chain(format!("{peer} does not share its screen")));
        }
        Ok(params)
    }

    fn receiver_params(&self, peer: &PublicKey) -> Result<ScreenParams, Error> {
        let params = self.peer_params(peer)?;
        if !params.views || !params.receives {
            return Err(Error::Chain(format!("{peer} does not take a cast")));
        }
        Ok(params)
    }

    fn peer_params(&self, peer: &PublicKey) -> Result<ScreenParams, Error> {
        if !self.same_person(peer) {
            return Err(Error::Chain(format!("{peer} is not one of my devices")));
        }
        self.daemon
            .registry()
            .peers()
            .into_iter()
            .find(|caps| caps.peer == *peer)
            .and_then(|caps| {
                caps.capabilities
                    .iter()
                    .find_map(ScreenParams::from_descriptor)
            })
            .ok_or_else(|| Error::Chain(format!("{peer} offers no screen")))
    }

    fn same_person(&self, peer: &PublicKey) -> bool {
        matches!(
            self.daemon.recognise(peer),
            Ok(Recognition::MyBinding { .. })
        )
    }

    /// The viewer's stream task: reassembles frames and forwards control.
    async fn run_watch(
        self: Arc<Self>,
        key: (PublicKey, String),
        mut stream: FramedStream,
        frames: mpsc::Sender<Viewed>,
        mut control: mpsc::UnboundedReceiver<ShareControl>,
    ) {
        let cap = lock(&self.watching).get(&key).map_or(MIN_FRAME_CAP, |w| {
            frame_cap(w.started.width, w.started.height)
        });
        let mut pending = Reassembly::new(cap);
        // Set when the viewer fell behind: frames are dropped until the next
        // keyframe, which the sharer has been asked for, so the decoder never
        // sees a frame whose reference it missed.
        let mut waiting_key = false;
        let reason = loop {
            tokio::select! {
                incoming = stream.recv::<ScreenMessage>() => {
                    tracing::trace!(peer = %key.0, ?incoming, "from the sharer");
                    match incoming {
                        Ok(Some(ScreenMessage::Frame(frame))) => {
                            let whole = match pending.push(frame) {
                                Ok(Some(whole)) => whole,
                                Ok(None) => continue,
                                Err(why) => {
                                    tracing::warn!(peer = %key.0, why, "the sharer's frame is out of bounds; ending the share");
                                    stream.reset(lp_core::frame::MALFORMED_CODE);
                                    break StopReason::Error;
                                }
                            };
                            if let Some(w) = lock(&self.watching).get_mut(&key) {
                                w.frames += 1;
                            }
                            if waiting_key && !whole.key {
                                continue;
                            }
                            match frames.try_send(Viewed::Frame { seq: whole.seq, at: whole.at, key: whole.key, bytes: whole.bytes }) {
                                Ok(()) => waiting_key = false,
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    // The viewer is behind: drop until a keyframe arrives
                                    // and the queue has room, asking for one now.
                                    waiting_key = true;
                                    if stream.send(&ScreenMessage::Keyframe).await.is_err() {
                                        break StopReason::Error;
                                    }
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    // The viewer went away: tell the sharer.
                                    let _ = stream.send(&ScreenMessage::Stop(Stop { reason: StopReason::Viewer })).await;
                                    stream.finish();
                                    break StopReason::Viewer;
                                }
                            }
                        }
                        Ok(Some(ScreenMessage::Audio(Audio { seq, at, bytes }))) => {
                            if let Some(w) = lock(&self.watching).get_mut(&key) {
                                w.audio += 1;
                            }
                            // Sound is small and keeps its place by its clock; a
                            // viewer that cannot keep up loses a packet, not the share.
                            let sent = frames.try_send(Viewed::Audio { seq, at, bytes: bytes.0 });
                            tracing::trace!(peer = %key.0, ok = sent.is_ok(), "audio to the viewer");
                            match sent {
                                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    let _ = stream.send(&ScreenMessage::Stop(Stop { reason: StopReason::Viewer })).await;
                                    stream.finish();
                                    break StopReason::Viewer;
                                }
                            }
                        }
                        Ok(Some(ScreenMessage::Stop(Stop { reason }))) => break reason,
                        Ok(Some(other)) => {
                            tracing::debug!(?other, "unexpected message from the sharer");
                        }
                        Ok(None) | Err(_) => break StopReason::Error,
                    }
                }
                wanted = control.recv() => {
                    match wanted {
                        Some(ShareControl::Keyframe) => {
                            if stream.send(&ScreenMessage::Keyframe).await.is_err() {
                                break StopReason::Error;
                            }
                        }
                        Some(ShareControl::Stop) | None => {
                            let _ = stream.send(&ScreenMessage::Stop(Stop { reason: StopReason::Viewer })).await;
                            stream.finish();
                            break StopReason::Viewer;
                        }
                    }
                }
            }
        };
        lock(&self.watching).remove(&key);
        let _ = frames.send(Viewed::Stopped(reason)).await;
        tracing::info!(peer = %key.0, source = %key.1, %reason, "stopped watching");
    }

    /// One incoming stream: a `displays` question or a `start` for the
    /// sharer, an `offer` for the receiver.
    async fn serve(self: Arc<Self>, peer: PublicKey, mut stream: FramedStream) {
        if !self.same_person(&peer) {
            tracing::warn!(%peer, "screen stream from a device that is not mine");
            stream.reset(lp_core::frame::REFUSED_CODE);
            return;
        }
        // The first message says what the stream is for.
        let first = match stream.recv::<ScreenMessage>().await {
            Ok(Some(m)) => m,
            _ => return,
        };
        let start = match first {
            ScreenMessage::Offer(offer) => {
                self.receive(peer, stream, offer).await;
                return;
            }
            ScreenMessage::Displays(_) => {
                let Some(capture) = self.capture.clone() else {
                    stream.reset(lp_core::frame::REFUSED_CODE);
                    return;
                };
                let windows = capture.windows();
                let _ = stream
                    .send(&ScreenMessage::Displays(Displays {
                        displays: Some(capture.displays()),
                        windows: (!windows.is_empty()).then_some(windows),
                    }))
                    .await;
                stream.finish();
                return;
            }
            ScreenMessage::Start(start) => start,
            other => {
                tracing::warn!(%peer, ?other, "screen stream opened with the wrong message");
                stream.reset(lp_core::frame::MALFORMED_CODE);
                return;
            }
        };
        let Some(capture) = self.capture.clone() else {
            stream.reset(lp_core::frame::REFUSED_CODE);
            return;
        };
        let (chosen, kind) = match (&start.window, &start.display) {
            (Some(id), _) => match capture.windows().iter().find(|w| &w.id == id) {
                Some(w) => (Source::window(w), SourceKind::Window),
                None => {
                    refuse(&mut stream, RefuseReason::NoDisplay).await;
                    return;
                }
            },
            (None, Some(id)) => match capture.displays().iter().find(|d| &d.id == id) {
                Some(d) => (Source::display(d), SourceKind::Display),
                None => {
                    refuse(&mut stream, RefuseReason::NoDisplay).await;
                    return;
                }
            },
            (None, None) => match primary(&capture.displays()) {
                Some(d) => (Source::display(d), SourceKind::Display),
                None => {
                    refuse(&mut stream, RefuseReason::NoDisplay).await;
                    return;
                }
            },
        };
        let (settings, share) = match self
            .start_share(&capture, &peer, &chosen, kind, &start, false)
            .await
        {
            Ok(started) => started,
            Err(reason) => {
                refuse(&mut stream, reason).await;
                return;
            }
        };
        self.run_started(&peer, settings, share, stream, false)
            .await;
    }

    /// The receiver's side of a cast: holds `offer` until whoever shows
    /// pictures here takes or declines it, answers `start` or `refused`, and
    /// then watches as for a share this device asked for.
    async fn receive(self: Arc<Self>, peer: PublicKey, mut stream: FramedStream, offer: Offer) {
        let Some(viewer) = self.viewer.clone() else {
            refuse(&mut stream, RefuseReason::NoDecoder).await;
            return;
        };
        let formats: Vec<Format> = viewer
            .decodes
            .iter()
            .copied()
            .filter(|f| offer.formats.contains(f))
            .collect();
        if formats.is_empty() {
            refuse(&mut stream, RefuseReason::NoDecoder).await;
            return;
        }
        let on_offer = lock(&self.on_offer).clone();
        let Some(on_offer) = on_offer else {
            tracing::info!(%peer, "a cast was offered but nothing here shows one");
            refuse(&mut stream, RefuseReason::Declined).await;
            return;
        };
        let (decide_tx, decide) = oneshot::channel();
        let busy = match lock(&self.pending).entry(peer) {
            std::collections::hash_map::Entry::Occupied(_) => true,
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(PendingOffer {
                    offer: offer.clone(),
                    decide: Some(decide_tx),
                });
                false
            }
        };
        if busy {
            refuse(&mut stream, RefuseReason::Busy).await;
            return;
        }
        tracing::info!(%peer, source = %offer.source.id, kind = %offer.source.kind, "a cast is offered");
        on_offer(peer, offer.clone());
        let decision = tokio::time::timeout(OFFER_TIMEOUT, decide).await;
        lock(&self.pending).remove(&peer);
        let (audio, reply) = match decision {
            Ok(Ok(Decision::Accept { audio, reply })) => (audio, reply),
            Ok(Ok(Decision::Decline)) | Ok(Err(_)) | Err(_) => {
                tracing::info!(%peer, "the cast was declined");
                refuse(&mut stream, RefuseReason::Declined).await;
                return;
            }
        };
        let start = Start {
            display: None,
            window: None,
            formats,
            width: offer.source.width.min(viewer.max_width),
            height: offer.source.height.min(viewer.max_height),
            frame_rate: viewer.max_frame_rate.min(offer.max_frame_rate.max(1)),
            audio: if audio {
                viewer
                    .audio_decodes
                    .iter()
                    .copied()
                    .filter(|f| offer.audio.contains(f))
                    .collect()
            } else {
                Vec::new()
            },
        };
        if let Err(e) = stream.send(&ScreenMessage::Start(start.clone())).await {
            let _ = reply.send(Err(e));
            return;
        }
        let started = match self
            .await_started(&peer, &mut stream, &start, Some(&offer.source))
            .await
        {
            Ok(started) => started,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        let _ = reply.send(Ok(self.watching(&peer, started, stream, true)));
    }

    /// Picks the formats and the size for `start` on `chosen`, and starts
    /// the capture, which may block on the platform's consent dialog.
    async fn start_share(
        &self,
        capture: &Arc<dyn Capture>,
        peer: &PublicKey,
        chosen: &Source,
        kind: SourceKind,
        start: &Start,
        cast: bool,
    ) -> Result<(Settings, Share), RefuseReason> {
        let params = capture.params();
        if !params.shares {
            return Err(RefuseReason::Consent);
        }
        // The first format of the viewer's offer this device encodes now.
        let format = start
            .formats
            .iter()
            .copied()
            .find(|f| params.encodes.contains(f))
            .ok_or(RefuseReason::NoEncoder)?;
        // And of its audio formats, when it asked for sound; none is silence.
        let audio = start
            .audio
            .iter()
            .copied()
            .find(|f| params.audio_encodes.contains(f));
        if lock(&self.sharing).contains_key(&chosen.id) {
            return Err(RefuseReason::Busy);
        }
        let (width, height) = fit(chosen.width, chosen.height, start.width, start.height);
        let settings = Settings {
            display: chosen.id.clone(),
            kind,
            format,
            audio,
            width,
            height,
            frame_rate: start.frame_rate.clamp(1, MAX_FRAME_RATE),
            viewer: self
                .daemon
                .device_display_name(peer)
                .unwrap_or_else(|| peer.fingerprint()),
            cast,
        };
        let starting = {
            let capture = capture.clone();
            let settings = settings.clone();
            tokio::task::spawn_blocking(move || capture.start(&settings))
        };
        match starting.await {
            Ok(Ok(share)) => Ok((settings, share)),
            Ok(Err(reason)) => Err(reason),
            Err(_) => Err(RefuseReason::Consent),
        }
    }

    /// Records a share this device runs, sends `started`, forwards frames
    /// and sound until it ends, and cleans up; returns why it ended.
    async fn run_started(
        &self,
        peer: &PublicKey,
        settings: Settings,
        share: Share,
        mut stream: FramedStream,
        cast: bool,
    ) -> StopReason {
        let id = settings.display.clone();
        let (stop_tx, stop_rx) = mpsc::unbounded_channel();
        let busy = {
            let mut sharing = lock(&self.sharing);
            if sharing.contains_key(&id) {
                true
            } else {
                sharing.insert(
                    id.clone(),
                    SharingState {
                        peer: *peer,
                        started: settings.started(),
                        frames: 0,
                        dropped: 0,
                        audio: 0,
                        cast,
                        stop: stop_tx,
                    },
                );
                false
            }
        };
        if busy {
            let _ = share.control.send(ShareControl::Stop);
            refuse(&mut stream, RefuseReason::Busy).await;
            return StopReason::Error;
        }
        crate::grants::record_implicit_for(&self.daemon, peer, SCREEN_TYPE);
        // A display's place on the screen maps the viewer's positions; a
        // window has none and is never driven.
        let region = match settings.kind {
            SourceKind::Display => share
                .region
                .or_else(|| self.capture.as_ref().and_then(|c| c.region(&id))),
            SourceKind::Window => None,
        };
        if let (Some(region), Some(input)) = (region, self.daemon.input_sync()) {
            input.set_region(peer, Some(region));
        }
        if stream
            .send(&ScreenMessage::Started(settings.started()))
            .await
            .is_err()
        {
            lock(&self.sharing).remove(&id);
            let _ = share.control.send(ShareControl::Stop);
            return StopReason::Error;
        }
        tracing::info!(%peer, source = %id, kind = %settings.kind, width = settings.width, height = settings.height, rate = settings.frame_rate, audio = ?settings.audio, cast, "sharing");
        let reason = self.run_share(peer, &id, stream, share, stop_rx).await;
        lock(&self.sharing).remove(&id);
        if let Some(input) = self.daemon.input_sync() {
            input.set_region(peer, None);
        }
        tracing::info!(%peer, source = %id, %reason, "share ended");
        reason
    }

    /// Forwards frames while the share lasts; returns why it ended.
    async fn run_share(
        &self,
        peer: &PublicKey,
        display: &str,
        mut stream: FramedStream,
        mut share: Share,
        mut stop: mpsc::UnboundedReceiver<StopReason>,
    ) -> StopReason {
        let mut seq: u64 = 0;
        let mut audio_seq: u64 = 0;
        // After a drop, deltas are useless until a keyframe comes.
        let mut need_key = false;
        let started = Instant::now();
        let now = || u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let reason = loop {
            tokio::select! {
                captured = share.frames.recv() => {
                    let Some(captured) = captured else { break StopReason::Error };
                    // What piled up while the last frame was written: the
                    // frames are brought up to date, the sound all goes.
                    let mut backlog = Vec::new();
                    let mut packets = Vec::new();
                    let mut ended = None;
                    let mut next = Some(captured);
                    while let Some(item) = next.take() {
                        match item {
                            Captured::Frame(f) => backlog.push(f),
                            Captured::Audio(a) => packets.push(a),
                            Captured::Ended(reason) => {
                                ended = Some(reason);
                                break;
                            }
                        }
                        next = share.frames.try_recv().ok();
                    }
                    let mut failed = false;
                    for packet in packets {
                        let at = if packet.at == 0 { now() } else { packet.at };
                        let message = ScreenMessage::Audio(Audio { seq: audio_seq, at, bytes: lp_core::wire::Bytes(packet.bytes) });
                        if stream.send(&message).await.is_err() {
                            failed = true;
                            break;
                        }
                        audio_seq += 1;
                    }
                    if failed {
                        break StopReason::Error;
                    }
                    let (send, dropped) = if backlog.is_empty() {
                        (Vec::new(), 0)
                    } else {
                        select_backlog(backlog, &mut need_key)
                    };
                    if dropped > 0 && need_key {
                        let _ = share.control.send(ShareControl::Keyframe);
                    }
                    for frame in send {
                        let at = if frame.at == 0 { now() } else { frame.at };
                        for chunk in chunk_frame(seq, at, frame.key, &frame.bytes) {
                            if stream.send(&ScreenMessage::Frame(chunk)).await.is_err() {
                                failed = true;
                                break;
                            }
                        }
                        if failed {
                            break;
                        }
                        seq += 1;
                    }
                    if failed {
                        break StopReason::Error;
                    }
                    let _ = &now;
                    let dropped = dropped as u64;
                    if let Some(s) = lock(&self.sharing).get_mut(display) {
                        s.frames = seq;
                        s.dropped += dropped;
                        s.audio = audio_seq;
                    }
                    if let Some(reason) = ended {
                        break reason;
                    }
                }
                incoming = stream.recv::<ScreenMessage>() => {
                    match incoming {
                        Ok(Some(ScreenMessage::Keyframe)) => {
                            let _ = share.control.send(ShareControl::Keyframe);
                        }
                        Ok(Some(ScreenMessage::Stop(Stop { reason }))) => break reason,
                        Ok(Some(other)) => {
                            tracing::debug!(%peer, ?other, "unexpected message from the viewer");
                        }
                        Ok(None) | Err(_) => break StopReason::Viewer,
                    }
                }
                asked = stop.recv() => {
                    break asked.unwrap_or(StopReason::Sharer);
                }
            }
        };
        let _ = share.control.send(ShareControl::Stop);
        if !matches!(reason, StopReason::Viewer) {
            let _ = stream.send(&ScreenMessage::Stop(Stop { reason })).await;
        }
        stream.finish();
        reason
    }

    fn handler(self: &Arc<Self>) -> StreamHandler {
        let sync = self.clone();
        Arc::new(move |conn, stream| {
            let sync = sync.clone();
            Box::pin(async move {
                let peer = *conn.peer();
                sync.serve(peer, stream).await;
            })
        })
    }
}

/// Which frames of a backlog to send, and how many are dropped. One frame
/// is sent as it is. Of several, the stream held the sharer back, so the
/// picture is brought up to date: from the newest keyframe on the frames
/// form a chain a decoder can follow and all go, and those before it are
/// dropped; with no keyframe in the backlog every frame is dropped and
/// `need_key` is set, since a delta whose predecessor was dropped would
/// corrupt the picture. While `need_key` is set, deltas are dropped until a
/// keyframe arrives.
fn select_backlog(backlog: Vec<EncodedFrame>, need_key: &mut bool) -> (Vec<EncodedFrame>, usize) {
    let total = backlog.len();
    let from = if total == 1 && !*need_key {
        Some(0)
    } else {
        backlog.iter().rposition(|f| f.key)
    };
    match from {
        Some(i) => {
            *need_key = false;
            (backlog.into_iter().skip(i).collect(), i)
        }
        None => {
            *need_key = true;
            (Vec::new(), total)
        }
    }
}

/// The display a `start` or a cast without an id shares: the primary one,
/// else the first.
fn primary(displays: &[Display]) -> Option<&Display> {
    displays
        .iter()
        .find(|d| d.primary)
        .or_else(|| displays.first())
}

/// Answers a `start` or an `offer` with `refused` and ends the stream.
async fn refuse(stream: &mut FramedStream, reason: RefuseReason) {
    let _ = stream
        .send(&ScreenMessage::Refused(Refused { reason }))
        .await;
    stream.finish();
}

/// Whether ffmpeg decodes H.264 on this machine, which is what `lpview`
/// needs to show a share.
#[must_use]
pub fn probe_decoder() -> bool {
    std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-decoders"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .is_some_and(|d| d.contains(" h264 "))
}

/// The frame rate a viewer takes and asks for: 60 a second, which halves every
/// per-frame wait between a change and its picture (LP-177).
pub const VIEW_RATE: u32 = 60;

/// The viewer params a desktop daemon advertises: views when ffmpeg decodes
/// H.264, which `lpview` uses, at 4:4:4 first, since its software decoder
/// takes it and it keeps coloured text sharp, and at 4:2:0, up to
/// [`VIEW_RATE`]; plays a share's sound when ffmpeg decodes Opus too.
#[must_use]
pub fn viewer_params() -> Option<ScreenParams> {
    viewer_params_for(probe_decoder(), sound::probe_decoder())
}

/// [`viewer_params`] for a daemon that does or does not have a picture
/// decoder, and a sound decoder.
fn viewer_params_for(decodes: bool, plays: bool) -> Option<ScreenParams> {
    decodes.then(|| ScreenParams {
        views: true,
        decodes: vec![Format::H264_444, Format::H264],
        audio_decodes: if plays { sound::decodes() } else { Vec::new() },
        max_frame_rate: VIEW_RATE,
        ..ScreenParams::default()
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The `lpview` installed beside the daemon's own executable (`~/.local/bin`
/// on both desktops, `target/debug` in a checkout), if there is one.
#[must_use]
pub fn lpview_beside(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) {
        "lpview.exe"
    } else {
        "lpview"
    };
    let candidate = exe.parent()?.join(name);
    candidate.is_file().then_some(candidate)
}

/// What a desktop daemon does with an offered cast: starts `lpview --receive`
/// for the caster, which takes the offer over the control socket and shows
/// it (milestone 9 decision 6); `None` when no `lpview` is installed beside
/// the daemon, so the device does not advertise `receives`.
#[must_use]
pub fn lpview_launcher(locations: &lp_core::paths::Locations) -> Option<OnOffer> {
    let lpview = lpview_beside(&std::env::current_exe().ok()?)?;
    let (data_dir, runtime_dir) = (locations.data_dir.clone(), locations.runtime_dir.clone());
    tracing::info!(lpview = %lpview.display(), "casts offered to this device open in lpview");
    Some(Arc::new(move |peer, offer| {
        let name = offer
            .source
            .name
            .clone()
            .unwrap_or_else(|| offer.source.id.clone());
        match std::process::Command::new(&lpview)
            .arg("--receive")
            .arg("--data-dir")
            .arg(&data_dir)
            .arg("--runtime-dir")
            .arg(&runtime_dir)
            .arg(peer.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            // Its complaints go where the daemon's do: the journal, or the
            // terminal of a daemon run by hand.
            .stderr(std::process::Stdio::inherit())
            .spawn()
        {
            Ok(_) => tracing::info!(%peer, source = %name, "lpview started for an offered cast"),
            Err(e) => {
                tracing::warn!(%peer, %e, "cannot start lpview for an offered cast; it will be declined");
            }
        }
    }))
}

/// An in-memory capture for tests and `LIGATURE_FAKE_SCREEN=1`: fixed
/// displays, frames the test emits or a generator paces at the agreed rate,
/// and a record of what was asked.
pub struct FakeCapture {
    shares: Mutex<bool>,
    encodes: Mutex<Vec<Format>>,
    audio_encodes: Mutex<Vec<AudioFormat>>,
    displays: Vec<Display>,
    windows: Vec<Window>,
    /// The settings of the last share started, for tests.
    last: Mutex<Option<Settings>>,
    /// Running shares by display or window: where emitted frames go, until
    /// the share is stopped or ended.
    running: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Captured>>>>,
    keyframe_requests: Arc<Mutex<u64>>,
    stops: Arc<Mutex<u64>>,
    /// Refuse the next start with this.
    refuse_next: Mutex<Option<RefuseReason>>,
    /// Produce synthetic frames at the agreed rate.
    generate: bool,
}

impl FakeCapture {
    /// A capture of `displays` that shares and produces frames only when the
    /// test emits them.
    #[must_use]
    pub fn new(displays: Vec<Display>) -> Arc<Self> {
        Arc::new(Self::with(displays, Vec::new(), false))
    }

    /// The same, with `windows` it can share by id.
    #[must_use]
    pub fn with_windows(displays: Vec<Display>, windows: Vec<Window>) -> Arc<Self> {
        Arc::new(Self::with(displays, windows, false))
    }

    /// A capture of one 1920 by 1080 display that generates synthetic frames
    /// at the agreed rate, a keyframe every two seconds and on request: what
    /// `LIGATURE_FAKE_SCREEN=1` attaches.
    #[must_use]
    pub fn generating() -> Arc<Self> {
        Arc::new(Self::with(
            vec![Display {
                id: "fake-1".to_owned(),
                name: Some("Fake display".to_owned()),
                width: 1920,
                height: 1080,
                primary: true,
                scale: None,
            }],
            Vec::new(),
            true,
        ))
    }

    fn with(displays: Vec<Display>, windows: Vec<Window>, generate: bool) -> Self {
        Self {
            shares: Mutex::new(true),
            encodes: Mutex::new(vec![Format::H264]),
            audio_encodes: Mutex::new(if generate {
                vec![AudioFormat::OPUS_STEREO]
            } else {
                Vec::new()
            }),
            displays,
            windows,
            last: Mutex::new(None),
            running: Arc::new(Mutex::new(HashMap::new())),
            keyframe_requests: Arc::new(Mutex::new(0)),
            stops: Arc::new(Mutex::new(0)),
            refuse_next: Mutex::new(None),
            generate,
        }
    }

    /// Switches sharing on or off, as a platform permission would.
    pub fn set_shares(&self, shares: bool) {
        *lock(&self.shares) = shares;
    }

    /// Sets the formats it says it encodes.
    pub fn set_encodes(&self, formats: Vec<Format>) {
        *lock(&self.encodes) = formats;
    }

    /// Sets the audio formats it says it encodes; none by default.
    pub fn set_audio_encodes(&self, formats: Vec<AudioFormat>) {
        *lock(&self.audio_encodes) = formats;
    }

    /// The settings of the last share started, if any.
    #[must_use]
    pub fn last_settings(&self) -> Option<Settings> {
        lock(&self.last).clone()
    }

    /// Delivers a packet of sound on the share of `display`; false if none
    /// runs.
    pub fn emit_audio(&self, display: &str, packet: AudioPacket) -> bool {
        lock(&self.running)
            .get(display)
            .is_some_and(|tx| tx.send(Captured::Audio(packet)).is_ok())
    }

    /// Refuses the next start with `reason`.
    pub fn refuse_next(&self, reason: RefuseReason) {
        *lock(&self.refuse_next) = Some(reason);
    }

    /// Delivers a frame on the share of `display`; false if none runs.
    pub fn emit(&self, display: &str, frame: EncodedFrame) -> bool {
        lock(&self.running)
            .get(display)
            .is_some_and(|tx| tx.send(Captured::Frame(frame)).is_ok())
    }

    /// Ends the share of `display` from the platform's side.
    pub fn end(&self, display: &str, reason: StopReason) {
        if let Some(tx) = lock(&self.running).remove(display) {
            let _ = tx.send(Captured::Ended(reason));
        }
    }

    /// How many keyframes were asked for, over every share.
    #[must_use]
    pub fn keyframe_requests(&self) -> u64 {
        *lock(&self.keyframe_requests)
    }

    /// How many shares the sync stopped.
    #[must_use]
    pub fn stops(&self) -> u64 {
        *lock(&self.stops)
    }

    /// The displays with a share running.
    #[must_use]
    pub fn running(&self) -> Vec<String> {
        let mut ids: Vec<String> = lock(&self.running).keys().cloned().collect();
        ids.sort();
        ids
    }
}

impl Capture for FakeCapture {
    fn params(&self) -> ScreenParams {
        ScreenParams {
            shares: *lock(&self.shares),
            encodes: lock(&self.encodes).clone(),
            audio_encodes: lock(&self.audio_encodes).clone(),
            ..ScreenParams::default()
        }
    }

    fn displays(&self) -> Vec<Display> {
        self.displays.clone()
    }

    fn windows(&self) -> Vec<Window> {
        self.windows.clone()
    }

    /// The fake's picker chooses its first window.
    fn pick(&self, kind: SourceKind) -> Result<Source, RefuseReason> {
        match (kind, self.windows.first()) {
            (SourceKind::Window, Some(w)) => Ok(Source::window(w)),
            _ => Err(RefuseReason::NoDisplay),
        }
    }

    fn start(&self, settings: &Settings) -> Result<Share, RefuseReason> {
        if let Some(reason) = lock(&self.refuse_next).take() {
            return Err(reason);
        }
        *lock(&self.last) = Some(settings.clone());
        let (frames_tx, frames) = mpsc::unbounded_channel();
        let (control, mut control_rx) = mpsc::unbounded_channel::<ShareControl>();
        lock(&self.running).insert(settings.display.clone(), frames_tx.clone());
        let running = Arc::new(Mutex::new(true));
        // A keyframe was asked for since the generator last looked.
        let wanted = Arc::new(Mutex::new(false));
        let (requests, stops, running_c, wanted_c, shares, id) = (
            self.keyframe_requests.clone(),
            self.stops.clone(),
            running.clone(),
            wanted.clone(),
            self.running.clone(),
            settings.display.clone(),
        );
        tokio::spawn(async move {
            while let Some(c) = control_rx.recv().await {
                match c {
                    ShareControl::Keyframe => {
                        *lock(&requests) += 1;
                        *lock(&wanted_c) = true;
                    }
                    ShareControl::Stop => {
                        *lock(&stops) += 1;
                        break;
                    }
                }
            }
            *lock(&running_c) = false;
            lock(&shares).remove(&id);
        });
        if self.generate && settings.audio.is_some() {
            // Fifty packets a second of something, as Opus would be.
            let (running, frames_tx) = (running.clone(), frames_tx.clone());
            tokio::spawn(async move {
                let started = Instant::now();
                let mut interval = tokio::time::interval(Duration::from_millis(20));
                let mut n: u64 = 0;
                while *lock(&running) {
                    interval.tick().await;
                    let packet = AudioPacket {
                        at: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                        bytes: (0..160).map(|i| ((i as u64 + n) % 251) as u8).collect(),
                    };
                    if frames_tx.send(Captured::Audio(packet)).is_err() {
                        break;
                    }
                    n += 1;
                }
            });
        }
        if self.generate {
            let rate = u64::from(settings.frame_rate.max(1));
            let period = Duration::from_micros(1_000_000 / rate);
            let (width, height) = (settings.width, settings.height);
            tokio::spawn(async move {
                let started = Instant::now();
                let mut n: u64 = 0;
                let mut interval = tokio::time::interval(period);
                while *lock(&running) {
                    interval.tick().await;
                    let key = n.is_multiple_of(2 * rate) || std::mem::take(&mut *lock(&wanted));
                    // A synthetic access unit: a start code, a byte that says
                    // key or delta, a payload of a keyframe's or a delta's size,
                    // and the picture size.
                    let size = if key { 200_000 } else { 8_000 };
                    let mut bytes = vec![0, 0, 0, 1, u8::from(key)];
                    bytes.extend((0..size).map(|i| ((i as u64 + n) % 251) as u8));
                    bytes.extend_from_slice(&width.to_be_bytes());
                    bytes.extend_from_slice(&height.to_be_bytes());
                    let frame = EncodedFrame {
                        at: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                        key,
                        bytes,
                    };
                    if frames_tx.send(Captured::Frame(frame)).is_err() {
                        break;
                    }
                    n += 1;
                }
            });
        }
        Ok(Share {
            frames,
            control,
            region: None,
        })
    }
}

/// Whether what the sharer settled on is within what the viewer asked for
/// (LP-186): a picture at least one pixel and no larger, a rate no faster, a
/// format from the offer, a sound format from the offer or none, and the
/// display or window asked for where one was, or the source `offered` when
/// the start answered a cast's offer. A sharer is one of the person's own
/// devices, but a buggy or compromised one must not choose the viewer's
/// buffers for it.
fn check_started(
    start: &Start,
    started: &Started,
    offered: Option<&Source>,
) -> Result<(), &'static str> {
    if started.width == 0 || started.height == 0 {
        return Err("an empty picture");
    }
    if started.width > start.width || started.height > start.height {
        return Err("a picture larger than asked for");
    }
    if started.frame_rate == 0 || started.frame_rate > start.frame_rate {
        return Err("a frame rate outside what was asked for");
    }
    if !start.formats.contains(&started.format) {
        return Err("a format that was not offered");
    }
    if let Some(audio) = started.audio
        && !start.audio.contains(&audio)
    {
        return Err("an audio format that was not offered");
    }
    if started.display.is_some() == started.window.is_some() {
        return Err("not exactly one of a display and a window");
    }
    let id = started.source_id();
    if id.is_empty() || id.chars().count() > 128 {
        return Err("a source name that is empty or too long");
    }
    if let Some(source) = offered {
        if id != source.id || started.kind() != source.kind {
            return Err("a source other than the one offered");
        }
    } else if let Some(asked) = &start.window {
        if started.window.as_deref() != Some(asked) {
            return Err("a window other than the one asked for");
        }
    } else if let Some(asked) = &start.display
        && started.display.as_deref() != Some(asked)
    {
        return Err("a display other than the one asked for");
    } else if start.display.is_none() && started.window.is_some() {
        return Err("a window where a display was asked for");
    }
    Ok(())
}

/// The most bytes a reassembled frame may hold: the picture's raw size, which
/// no encoded frame of it exceeds, and at least [`MIN_FRAME_CAP`].
fn frame_cap(width: u32, height: u32) -> usize {
    let raw = u64::from(width) * u64::from(height) * 4;
    usize::try_from(raw)
        .unwrap_or(usize::MAX)
        .max(MIN_FRAME_CAP)
}

/// A frame received whole, its chunks joined.
#[derive(Debug, PartialEq, Eq)]
struct WholeFrame {
    seq: u64,
    at: u64,
    key: bool,
    bytes: Vec<u8>,
}

/// Joins a frame's chunks, within bounds: no chunk over [`CHUNK_BYTES`] and
/// no frame over its cap, since a sharer that never sends `last` would
/// otherwise grow the viewer's memory without end (LP-186).
struct Reassembly {
    cap: usize,
    pending: Option<WholeFrame>,
}

impl Reassembly {
    fn new(cap: usize) -> Self {
        Self { cap, pending: None }
    }

    /// Takes one chunk; returns the frame once its last chunk is in.
    fn push(&mut self, frame: Frame) -> Result<Option<WholeFrame>, &'static str> {
        if frame.bytes.0.len() > CHUNK_BYTES {
            self.pending = None;
            return Err("a chunk over the chunk bound");
        }
        let whole = match &mut self.pending {
            Some(w) if w.seq == frame.seq => w,
            _ => {
                self.pending = Some(WholeFrame {
                    seq: frame.seq,
                    at: frame.at,
                    key: frame.key,
                    bytes: Vec::new(),
                });
                self.pending.as_mut().expect("just set")
            }
        };
        if whole.bytes.len() + frame.bytes.0.len() > self.cap {
            self.pending = None;
            return Err("a frame over the picture's size");
        }
        whole.bytes.extend_from_slice(&frame.bytes.0);
        if frame.last {
            return Ok(self.pending.take());
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked() -> Start {
        Start {
            display: Some("1".to_owned()),
            window: None,
            formats: vec![Format::H264],
            width: 1280,
            height: 720,
            frame_rate: 60,
            audio: vec![AudioFormat::OPUS_STEREO],
        }
    }

    fn settled() -> Started {
        Started {
            display: Some("1".to_owned()),
            window: None,
            format: Format::H264,
            audio: None,
            width: 1280,
            height: 720,
            frame_rate: 60,
        }
    }

    #[test]
    fn a_started_within_the_request_passes_and_one_outside_it_is_refused() {
        // LP-186: the sharer chooses nothing the viewer did not ask for.
        assert_eq!(check_started(&asked(), &settled(), None), Ok(()));
        let smaller = Started {
            width: 640,
            height: 360,
            frame_rate: 30,
            ..settled()
        };
        assert_eq!(check_started(&asked(), &smaller, None), Ok(()));
        let any_display = Start {
            display: None,
            ..asked()
        };
        assert_eq!(check_started(&any_display, &settled(), None), Ok(()));
        for (bad, why) in [
            (
                Started {
                    width: 0,
                    ..settled()
                },
                "empty",
            ),
            (
                Started {
                    height: 0,
                    ..settled()
                },
                "empty",
            ),
            (
                Started {
                    width: 1281,
                    ..settled()
                },
                "larger",
            ),
            (
                Started {
                    height: u32::MAX,
                    ..settled()
                },
                "larger",
            ),
            (
                Started {
                    frame_rate: 61,
                    ..settled()
                },
                "rate",
            ),
            (
                Started {
                    frame_rate: 0,
                    ..settled()
                },
                "rate",
            ),
            (
                Started {
                    format: Format::H264_444,
                    ..settled()
                },
                "format",
            ),
            (
                Started {
                    display: Some("2".to_owned()),
                    ..settled()
                },
                "display",
            ),
            (
                Started {
                    display: Some(String::new()),
                    ..settled()
                },
                "display",
            ),
            (
                Started {
                    display: Some("x".repeat(129)),
                    ..settled()
                },
                "display",
            ),
            (
                Started {
                    display: None,
                    ..settled()
                },
                "neither a display nor a window",
            ),
            (
                Started {
                    window: Some("1".to_owned()),
                    ..settled()
                },
                "both a display and a window",
            ),
            (
                Started {
                    audio: Some(AudioFormat::OPUS_MONO),
                    ..settled()
                },
                "an audio format not offered",
            ),
        ] {
            assert!(
                check_started(&asked(), &bad, None).is_err(),
                "{why}: {bad:?} should be refused"
            );
        }
        // Milestone 9: sound from the offer passes; a window asked for must
        // be the window started, and a display asked by default is never a
        // window.
        let with_sound = Started {
            audio: Some(AudioFormat::OPUS_STEREO),
            ..settled()
        };
        assert_eq!(check_started(&asked(), &with_sound, None), Ok(()));
        let window_asked = Start {
            display: None,
            window: Some("w".to_owned()),
            ..asked()
        };
        let window_started = Started {
            display: None,
            window: Some("w".to_owned()),
            ..settled()
        };
        assert_eq!(check_started(&window_asked, &window_started, None), Ok(()));
        assert!(check_started(&window_asked, &settled(), None).is_err());
        assert!(check_started(&any_display, &window_started, None).is_err());
        // The answer to an offer names nothing: the source offered must be
        // the one started.
        let answer = Start {
            display: None,
            ..asked()
        };
        let offered = Source {
            kind: SourceKind::Window,
            id: "w".to_owned(),
            name: None,
            app: None,
            width: 1600,
            height: 1000,
            scale: None,
        };
        assert_eq!(
            check_started(&answer, &window_started, Some(&offered)),
            Ok(())
        );
        assert!(check_started(&answer, &settled(), Some(&offered)).is_err());
        assert!(check_started(&answer, &window_started, None).is_err());
    }

    #[test]
    fn lpview_is_found_beside_the_daemon_or_not_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = dir.path().join("ligatured");
        std::fs::write(&daemon, b"").unwrap();
        assert_eq!(lpview_beside(&daemon), None);
        let lpview = dir.path().join(if cfg!(windows) {
            "lpview.exe"
        } else {
            "lpview"
        });
        std::fs::write(&lpview, b"").unwrap();
        assert_eq!(lpview_beside(&daemon), Some(lpview.clone()));
        // A directory of that name is not it.
        std::fs::remove_file(&lpview).unwrap();
        std::fs::create_dir(&lpview).unwrap();
        assert_eq!(lpview_beside(&daemon), None);
    }

    #[test]
    fn settings_become_started_by_their_kind() {
        let settings = Settings {
            display: "w-1".to_owned(),
            kind: SourceKind::Window,
            format: Format::H264,
            audio: Some(AudioFormat::OPUS_STEREO),
            width: 640,
            height: 480,
            frame_rate: 30,
            viewer: "mac".to_owned(),
            cast: true,
        };
        let started = settings.started();
        assert_eq!(started.window.as_deref(), Some("w-1"));
        assert_eq!(started.display, None);
        assert_eq!(started.kind(), SourceKind::Window);
        assert_eq!(started.audio, Some(AudioFormat::OPUS_STEREO));
        let display = Settings {
            kind: SourceKind::Display,
            ..settings
        }
        .started();
        assert_eq!(display.display.as_deref(), Some("w-1"));
        assert_eq!(display.window, None);
        let none: Vec<Display> = Vec::new();
        assert!(primary(&none).is_none());
    }

    fn chunk(seq: u64, last: bool, len: usize) -> Frame {
        Frame {
            seq,
            at: 7,
            key: seq == 1,
            last,
            bytes: lp_core::wire::Bytes(vec![0xAB; len]),
        }
    }

    #[test]
    fn a_frame_is_joined_from_its_chunks_within_its_bounds() {
        let mut r = Reassembly::new(1000);
        assert!(r.push(chunk(1, false, 400)).unwrap().is_none());
        let whole = r.push(chunk(1, true, 300)).unwrap().expect("whole");
        assert_eq!(
            (whole.seq, whole.at, whole.key, whole.bytes.len()),
            (1, 7, true, 700)
        );
        // A chunk of another frame drops what was pending.
        assert!(r.push(chunk(2, false, 100)).unwrap().is_none());
        let whole = r.push(chunk(3, true, 100)).unwrap().expect("whole");
        assert_eq!((whole.seq, whole.bytes.len()), (3, 100));
    }

    #[test]
    fn a_frame_over_the_cap_or_a_chunk_over_the_bound_is_refused() {
        let mut r = Reassembly::new(1000);
        assert!(r.push(chunk(1, false, 600)).unwrap().is_none());
        assert_eq!(
            r.push(chunk(1, false, 600)),
            Err("a frame over the picture's size")
        );
        // What was pending is gone with it.
        assert!(r.push(chunk(1, true, 10)).unwrap().is_some());
        let mut r = Reassembly::new(usize::MAX);
        assert_eq!(
            r.push(chunk(1, true, CHUNK_BYTES + 1)),
            Err("a chunk over the chunk bound")
        );
        assert_eq!(frame_cap(2, 2), MIN_FRAME_CAP);
        assert_eq!(frame_cap(1280, 720), 1280 * 720 * 4);
    }

    #[test]
    fn a_viewer_takes_and_asks_for_sixty_frames_a_second() {
        // LP-177: every per-frame wait halves at 60.
        assert_eq!(VIEW_RATE, 60);
        let params = viewer_params_for(true, true).expect("a decoder views");
        assert_eq!(params.max_frame_rate, VIEW_RATE);
        assert_eq!(params.audio_decodes, sound::decodes());
        assert!(
            viewer_params_for(true, false)
                .unwrap()
                .audio_decodes
                .is_empty()
        );
        assert!(viewer_params_for(false, true).is_none());
    }

    fn f(key: bool, n: u8) -> EncodedFrame {
        EncodedFrame {
            at: 0,
            key,
            bytes: vec![n],
        }
    }

    fn ids(frames: &[EncodedFrame]) -> Vec<u8> {
        frames.iter().map(|f| f.bytes[0]).collect()
    }

    #[test]
    fn a_backlog_is_sent_from_its_newest_keyframe() {
        let mut need = false;
        let (send, dropped) = select_backlog(vec![f(false, 1)], &mut need);
        assert_eq!((ids(&send), dropped, need), (vec![1], 0, false));
        // A burst: the keyframe and the delta after it both go.
        let (send, dropped) = select_backlog(vec![f(true, 1), f(false, 2)], &mut need);
        assert_eq!((ids(&send), dropped, need), (vec![1, 2], 0, false));
        // Frames before the newest keyframe are dropped.
        let (send, dropped) = select_backlog(
            vec![
                f(false, 1),
                f(true, 2),
                f(false, 3),
                f(true, 4),
                f(false, 5),
            ],
            &mut need,
        );
        assert_eq!((ids(&send), dropped, need), (vec![4, 5], 3, false));
        // No keyframe in a backlog: everything goes, and deltas wait for one.
        let (send, dropped) = select_backlog(vec![f(false, 1), f(false, 2)], &mut need);
        assert_eq!((ids(&send), dropped, need), (vec![], 2, true));
        let (send, dropped) = select_backlog(vec![f(false, 3)], &mut need);
        assert_eq!((ids(&send), dropped, need), (vec![], 1, true));
        let (send, dropped) = select_backlog(vec![f(true, 4)], &mut need);
        assert_eq!((ids(&send), dropped, need), (vec![4], 0, false));
    }
}
