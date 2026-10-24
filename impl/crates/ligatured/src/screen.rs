//! A device's screen watched live on another of the person's devices. Spec
//! chapter 05 section 3h; milestone 8 (LP-148).
//!
//! The sharer's platform is behind the [`Capture`] trait: it lists displays
//! and starts a share, which delivers encoded frames on a channel until it
//! ends. The viewer's side is a sink the daemon hands to whoever asked to
//! watch (`lpview` over the control socket, the phone through its node): the
//! reassembled frames and the end of the share, with a handle to ask for a
//! keyframe or to stop. The [`ScreenSync`] negotiates `start`, chunks frames
//! within the message bound, drops rather than queues when the stream holds
//! it back (and asks the encoder for a keyframe once it has dropped), answers
//! `keyframe`, and ends a share on every path: the viewer's `stop`, the
//! sharer's, the platform's withdrawn consent, a failed capture, the stream's
//! end and the connection's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_core::input::Region;
use lp_core::screen::{
    Codec, Display, Displays, Frame, RefuseReason, Refused, SCREEN_TYPE, ScreenMessage,
    ScreenParams, Start, Started, Stop, StopReason, chunk_frame, fit, screen_descriptor,
};
use lp_core::{Error, PublicKey};
use lp_identity::trust::Recognition;
use lp_transport::manager::{StreamHandler, open_on};
use lp_transport::stream::FramedStream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::daemon::Daemon;

pub mod encode;

/// How often the capture's flags are re-read for the manifest.
const ADVERTISE_POLL: Duration = Duration::from_secs(1);
/// How long the viewer waits for `started` or `refused`, which may wait on
/// the sharer's consent dialog.
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// The fastest frame rate a sharer agrees to.
const MAX_FRAME_RATE: u32 = 60;

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

/// What a running capture delivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// A frame.
    Frame(EncodedFrame),
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
    /// The display.
    pub display: String,
    /// The codec.
    pub codec: Codec,
    /// The picture's width.
    pub width: u32,
    /// The picture's height.
    pub height: u32,
    /// The frame rate.
    pub frame_rate: u32,
    /// The viewing device's name, for the platform's sharing indicator.
    pub viewer: String,
}

impl Settings {
    fn started(&self) -> Started {
        Started {
            display: self.display.clone(),
            codec: self.codec,
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

/// The sharer's platform: displays, and a share of one.
pub trait Capture: Send + Sync {
    /// Whether this device shares right now and with which codecs; `views`
    /// and the viewer bounds are the sync's, not the capture's.
    fn params(&self) -> ScreenParams;
    /// The displays this device can share.
    fn displays(&self) -> Vec<Display>;
    /// Where `display` lies in the whole screen, for a device with several.
    fn region(&self, _display: &str) -> Option<Region> {
        None
    }
    /// Starts a share with `settings`, which the sync has already fitted to
    /// the display and the viewer. Blocks on the platform's consent where it
    /// has one, so it runs off the async runtime.
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
    /// The share ended.
    Stopped(StopReason),
}

/// A share this device watches: what `watch` returns.
pub struct Watching {
    /// What the sharer settled on.
    pub started: Started,
    /// The frames and the end.
    pub frames: mpsc::UnboundedReceiver<Viewed>,
    /// Keyframe requests and the stop.
    pub control: mpsc::UnboundedSender<ShareControl>,
}

impl std::fmt::Debug for Watching {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Watching({:?})", self.started)
    }
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
}

/// What `lpctl screen status` shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenStatus {
    /// What this device advertises.
    pub shares: bool,
    /// What this device advertises.
    pub views: bool,
    /// The shares this device runs.
    pub sharing: Vec<Sharing>,
    /// The shares this device watches.
    pub watching: Vec<WatchingStatus>,
}

struct SharingState {
    peer: PublicKey,
    started: Started,
    frames: u64,
    dropped: u64,
    /// Ends the stream task from outside: a stop from the sharer's side.
    stop: mpsc::UnboundedSender<StopReason>,
}

struct WatchingState {
    peer: PublicKey,
    started: Started,
    frames: u64,
    /// A stop from outside the viewer: `lpctl screen stop`.
    stop: mpsc::UnboundedSender<ShareControl>,
}

/// The screen synchroniser for one daemon.
pub struct ScreenSync {
    daemon: Arc<Daemon>,
    capture: Option<Arc<dyn Capture>>,
    /// What this device takes as a viewer, if it views at all.
    viewer: Option<ScreenParams>,
    /// Shares running, by display id.
    sharing: Mutex<HashMap<String, SharingState>>,
    /// Shares watched, by sharer and display.
    watching: Mutex<HashMap<(PublicKey, String), WatchingState>>,
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
            sharing: Mutex::new(HashMap::new()),
            watching: Mutex::new(HashMap::new()),
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

    /// What this device advertises right now.
    #[must_use]
    pub fn params(&self) -> ScreenParams {
        let mut params = self.viewer.clone().unwrap_or_default();
        params.views = self.viewer.is_some();
        match self.capture.as_ref().map(|c| c.params()) {
            Some(capture) => {
                params.shares = capture.shares;
                if !params.views {
                    params.codecs = capture.codecs;
                }
            }
            None => params.shares = false,
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
            sharing: lock(&self.sharing)
                .values()
                .map(|s| Sharing {
                    peer: s.peer,
                    started: s.started.clone(),
                    frames: s.frames,
                    dropped: s.dropped,
                })
                .collect(),
            watching: lock(&self.watching)
                .values()
                .map(|w| WatchingStatus {
                    peer: w.peer,
                    started: w.started.clone(),
                    frames: w.frames,
                })
                .collect(),
        }
    }

    /// Asks `peer` what displays it has.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] when `peer` is not this person's device or
    /// does not share, and [`Error::Transport`] when the stream fails.
    pub async fn displays(&self, peer: &PublicKey) -> Result<Vec<Display>, Error> {
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
            Some(ScreenMessage::Displays(Displays {
                displays: Some(list),
            })) => Ok(list),
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
        if !sharer.codecs.contains(&start.codec) || !viewer.codecs.contains(&start.codec) {
            return Err(Error::Chain(format!(
                "{} is not a codec both sides have",
                start.codec
            )));
        }
        let start = Start {
            width: start.width.min(viewer.max_width),
            height: start.height.min(viewer.max_height),
            frame_rate: start.frame_rate.min(viewer.max_frame_rate),
            ..start
        };
        let mut stream = self.open(peer).await?;
        stream.send(&ScreenMessage::Start(start)).await?;
        let answer = tokio::time::timeout(START_TIMEOUT, stream.recv::<ScreenMessage>())
            .await
            .map_err(|_| Error::Transport(format!("{peer} did not answer start")))??;
        let started = match answer {
            Some(ScreenMessage::Started(started)) => started,
            Some(ScreenMessage::Refused(Refused { reason })) => {
                return Err(Error::Chain(format!("{peer} refused the share: {reason}")));
            }
            other => {
                stream.reset(lp_core::frame::MALFORMED_CODE);
                return Err(Error::Transport(format!(
                    "{peer} answered start with {other:?}"
                )));
            }
        };
        let (frames_tx, frames) = mpsc::unbounded_channel();
        let (control, control_rx) = mpsc::unbounded_channel();
        let key = (*peer, started.display.clone());
        lock(&self.watching).insert(
            key.clone(),
            WatchingState {
                peer: *peer,
                started: started.clone(),
                frames: 0,
                stop: control.clone(),
            },
        );
        tracing::info!(%peer, display = %started.display, width = started.width, height = started.height, "watching");
        let this = self.clone();
        tokio::spawn(async move { this.run_watch(key, stream, frames_tx, control_rx).await });
        Ok(Watching {
            started,
            frames,
            control,
        })
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
        if !self.same_person(peer) {
            return Err(Error::Chain(format!("{peer} is not one of my devices")));
        }
        let params = self
            .daemon
            .registry()
            .peers()
            .into_iter()
            .find(|caps| caps.peer == *peer)
            .and_then(|caps| {
                caps.capabilities
                    .iter()
                    .find_map(ScreenParams::from_descriptor)
            })
            .ok_or_else(|| Error::Chain(format!("{peer} offers no screen")))?;
        if !params.shares {
            return Err(Error::Chain(format!("{peer} does not share its screen")));
        }
        Ok(params)
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
        frames: mpsc::UnboundedSender<Viewed>,
        mut control: mpsc::UnboundedReceiver<ShareControl>,
    ) {
        let mut pending: Option<(u64, u64, bool, Vec<u8>)> = None;
        let reason = loop {
            tokio::select! {
                incoming = stream.recv::<ScreenMessage>() => {
                    match incoming {
                        Ok(Some(ScreenMessage::Frame(Frame { seq, at, key: is_key, last, bytes }))) => {
                            let buffer = match &mut pending {
                                Some((s, _, _, buf)) if *s == seq => buf,
                                _ => {
                                    pending = Some((seq, at, is_key, Vec::new()));
                                    &mut pending.as_mut().expect("just set").3
                                }
                            };
                            buffer.extend_from_slice(&bytes.0);
                            if last && let Some((seq, at, is_key, bytes)) = pending.take() {
                                if let Some(w) = lock(&self.watching).get_mut(&key) {
                                    w.frames += 1;
                                }
                                if frames.send(Viewed::Frame { seq, at, key: is_key, bytes }).is_err() {
                                    // The viewer went away: tell the sharer.
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
        let _ = frames.send(Viewed::Stopped(reason));
        tracing::info!(peer = %key.0, display = %key.1, %reason, "stopped watching");
    }

    /// The sharer's side of one stream: answers `displays`, runs one share.
    async fn serve(self: Arc<Self>, peer: PublicKey, mut stream: FramedStream) {
        let Some(capture) = self.capture.clone() else {
            stream.reset(lp_core::frame::REFUSED_CODE);
            return;
        };
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
            ScreenMessage::Displays(_) => {
                let list = capture.displays();
                let _ = stream
                    .send(&ScreenMessage::Displays(Displays {
                        displays: Some(list),
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
        let params = capture.params();
        if !params.shares {
            refuse(&mut stream, RefuseReason::Consent).await;
            return;
        }
        if !params.codecs.contains(&start.codec) {
            refuse(&mut stream, RefuseReason::NoEncoder).await;
            return;
        }
        let displays = capture.displays();
        let display = match &start.display {
            Some(id) => displays.iter().find(|d| &d.id == id),
            None => displays
                .iter()
                .find(|d| d.primary)
                .or_else(|| displays.first()),
        };
        let Some(chosen) = display.cloned() else {
            refuse(&mut stream, RefuseReason::NoDisplay).await;
            return;
        };
        if lock(&self.sharing).contains_key(&chosen.id) {
            refuse(&mut stream, RefuseReason::Busy).await;
            return;
        }
        let (width, height) = fit(chosen.width, chosen.height, start.width, start.height);
        let settings = Settings {
            display: chosen.id.clone(),
            codec: start.codec,
            width,
            height,
            frame_rate: start.frame_rate.clamp(1, MAX_FRAME_RATE),
            viewer: self
                .daemon
                .device_display_name(&peer)
                .unwrap_or_else(|| peer.fingerprint()),
        };
        // The capture may block on the platform's consent dialog.
        let starting = {
            let capture = capture.clone();
            let settings = settings.clone();
            tokio::task::spawn_blocking(move || capture.start(&settings))
        };
        let share = match starting.await {
            Ok(Ok(share)) => share,
            Ok(Err(reason)) => {
                refuse(&mut stream, reason).await;
                return;
            }
            Err(_) => {
                refuse(&mut stream, RefuseReason::Consent).await;
                return;
            }
        };
        let (stop_tx, stop_rx) = mpsc::unbounded_channel();
        let busy = {
            let mut sharing = lock(&self.sharing);
            if sharing.contains_key(&chosen.id) {
                true
            } else {
                sharing.insert(
                    chosen.id.clone(),
                    SharingState {
                        peer,
                        started: settings.started(),
                        frames: 0,
                        dropped: 0,
                        stop: stop_tx,
                    },
                );
                false
            }
        };
        if busy {
            let _ = share.control.send(ShareControl::Stop);
            refuse(&mut stream, RefuseReason::Busy).await;
            return;
        }
        crate::grants::record_implicit_for(&self.daemon, &peer, SCREEN_TYPE);
        let region = share.region.or_else(|| capture.region(&chosen.id));
        if let (Some(region), Some(input)) = (region, self.daemon.input_sync()) {
            input.set_region(&peer, Some(region));
        }
        if stream
            .send(&ScreenMessage::Started(settings.started()))
            .await
            .is_err()
        {
            lock(&self.sharing).remove(&chosen.id);
            let _ = share.control.send(ShareControl::Stop);
            return;
        }
        tracing::info!(%peer, display = %chosen.id, width, height, rate = settings.frame_rate, "sharing");
        let reason = self
            .run_share(&peer, &chosen.id, stream, share, stop_rx)
            .await;
        lock(&self.sharing).remove(&chosen.id);
        if let Some(input) = self.daemon.input_sync() {
            input.set_region(&peer, None);
        }
        tracing::info!(%peer, display = %chosen.id, %reason, "share ended");
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
        // After a drop, deltas are useless until a keyframe comes.
        let mut need_key = false;
        let started = Instant::now();
        let reason = loop {
            tokio::select! {
                captured = share.frames.recv() => {
                    let Some(captured) = captured else { break StopReason::Error };
                    let first = match captured {
                        Captured::Frame(f) => f,
                        Captured::Ended(reason) => break reason,
                    };
                    // What piled up while the last frame was written.
                    let mut backlog = vec![first];
                    let mut ended = None;
                    while let Ok(next) = share.frames.try_recv() {
                        match next {
                            Captured::Frame(f) => backlog.push(f),
                            Captured::Ended(reason) => {
                                ended = Some(reason);
                                break;
                            }
                        }
                    }
                    let (send, dropped) = select_backlog(backlog, &mut need_key);
                    if dropped > 0 && need_key {
                        let _ = share.control.send(ShareControl::Keyframe);
                    }
                    let mut failed = false;
                    for frame in send {
                        let at = if frame.at == 0 {
                            u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
                        } else {
                            frame.at
                        };
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
                    let dropped = dropped as u64;
                    if let Some(s) = lock(&self.sharing).get_mut(display) {
                        s.frames = seq;
                        s.dropped += dropped;
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

/// Answers `start` with `refused` and ends the stream.
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

/// The viewer params a desktop daemon advertises: views when ffmpeg decodes
/// H.264, which `lpview` uses.
#[must_use]
pub fn viewer_params() -> Option<ScreenParams> {
    probe_decoder().then(|| ScreenParams {
        views: true,
        ..ScreenParams::default()
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An in-memory capture for tests and `LIGATURE_FAKE_SCREEN=1`: fixed
/// displays, frames the test emits or a generator paces at the agreed rate,
/// and a record of what was asked.
pub struct FakeCapture {
    shares: Mutex<bool>,
    displays: Vec<Display>,
    /// Running shares by display: where emitted frames go.
    running: Mutex<HashMap<String, mpsc::UnboundedSender<Captured>>>,
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
        Arc::new(Self::with(displays, false))
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
            }],
            true,
        ))
    }

    fn with(displays: Vec<Display>, generate: bool) -> Self {
        Self {
            shares: Mutex::new(true),
            displays,
            running: Mutex::new(HashMap::new()),
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
            ..ScreenParams::default()
        }
    }

    fn displays(&self) -> Vec<Display> {
        self.displays.clone()
    }

    fn start(&self, settings: &Settings) -> Result<Share, RefuseReason> {
        if let Some(reason) = lock(&self.refuse_next).take() {
            return Err(reason);
        }
        let (frames_tx, frames) = mpsc::unbounded_channel();
        let (control, mut control_rx) = mpsc::unbounded_channel::<ShareControl>();
        lock(&self.running).insert(settings.display.clone(), frames_tx.clone());
        let running = Arc::new(Mutex::new(true));
        // A keyframe was asked for since the generator last looked.
        let wanted = Arc::new(Mutex::new(false));
        let (requests, stops, running_c, wanted_c) = (
            self.keyframe_requests.clone(),
            self.stops.clone(),
            running.clone(),
            wanted.clone(),
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
        });
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

#[cfg(test)]
mod tests {
    use super::*;

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
