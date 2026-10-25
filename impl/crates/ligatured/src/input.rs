//! One keyboard and mouse across a person's devices. Spec chapter 05 section
//! 3g; milestone 7 (LP-142).
//!
//! The daemon has two sides, each behind a trait a platform implements. A
//! [`Capture`] is the controller's own keyboard and mouse: it tells the sync
//! when the pointer hits an edge of the screen, and while taken it reports
//! what the user does and keeps it from the local desktop. An [`Inject`] is
//! the driven device's input system: it places the pointer, injects what it
//! is sent and lifts everything held. The [`InputSync`] follows the layout
//! (which edge leads to which device, rows in the store), runs the handoff
//! (`enter`, `ready`, `leave`), batches captured events per frame, scales
//! motion by the two screens' widths, tracks the driven pointer's position to
//! know when it reaches its entry edge, lifts everything on every way a drive
//! ends, honours the release chord (both Control keys) and measures the round
//! trip through `seq` and `injected`. A drive is one stream opened by the
//! controller; the stream's end is a release.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_core::input::{
    Edge, Enter, Events, INPUT_TYPE, Injected, InputEvent, InputMessage, InputParams, KEY_LEFTCTRL,
    KEY_RIGHTCTRL, Leave, Ready, Region, input_descriptor,
};
use lp_core::{Error, PublicKey};
use lp_identity::trust::Recognition;
use lp_transport::manager::{StreamHandler, open_on};
use lp_transport::stream::FramedStream;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::daemon::Daemon;

pub mod geometry;
pub mod mac;

/// How often the backends' flags are re-read for the manifest.
const ADVERTISE_POLL: Duration = Duration::from_secs(1);
/// One input frame: captured events within it travel as one batch.
const FRAME: Duration = Duration::from_millis(8);
/// How long the controller waits for `ready` after `enter`.
const READY_TIMEOUT: Duration = Duration::from_secs(3);
/// Every so many batches carries `seq`, for the round trip.
const MEASURE_EVERY: u64 = 16;
/// How many round-trip samples are kept.
const SAMPLES: usize = 256;
/// The most events one batch carries, the schema's `maxItems`.
const MAX_BATCH: usize = 256;

/// What the controller's platform reports.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    /// The pointer reached `edge` at `at` along it while not captured: a
    /// drive starts if the layout maps that edge.
    EdgeHit {
        /// The edge of this screen.
        edge: Edge,
        /// Where along it, a fraction from its top or left end.
        at: f64,
    },
    /// An event while captured, in this screen's pixels.
    Input(InputEvent),
    /// The platform took the input back itself (the compositor ended the
    /// capture): the drive ends as if the user had released it.
    Lost,
}

/// The controller's own keyboard and mouse.
pub trait Capture: Send + Sync {
    /// Whether this device can capture at all right now.
    fn provides(&self) -> bool;
    /// This screen's size in pixels, for scaling and for `at`.
    fn screen(&self) -> (u32, u32);
    /// The edges a drive may start from: the platform watches these.
    fn arm(&self, edges: &[Edge]);
    /// Takes the pointer and keyboard from the local desktop; events follow
    /// on [`Capture::subscribe`] until [`Capture::release`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the platform refuses.
    fn take(&self, edge: Edge) -> Result<(), Error>;
    /// Gives the pointer and keyboard back, placing the pointer on `edge` at
    /// `at` when the drive ended by an edge.
    fn release(&self, back_at: Option<(Edge, f64)>);
    /// Edge hits and captured events.
    fn subscribe(&self) -> broadcast::Receiver<CaptureEvent>;
}

/// The driven device's input system.
pub trait Inject: Send + Sync {
    /// The directions and forms this device takes right now.
    fn params(&self) -> InputParams;
    /// This screen's size in pixels, reported in `ready`.
    fn screen(&self) -> (u32, u32);
    /// A drive begins: place the pointer at `start`, fractions of the screen.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the platform refuses.
    fn begin(&self, start: (f64, f64)) -> Result<(), Error>;
    /// Injects one event; motion is already in this screen's pixels.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the platform refuses.
    fn inject(&self, event: &InputEvent) -> Result<(), Error>;
    /// Lifts every key and button still held and ends the drive.
    fn end(&self);
}

/// Round-trip figures from the batches that carried `seq`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct RoundTrip {
    /// How many batches were measured.
    pub samples: usize,
    /// The median, in milliseconds.
    pub median_ms: f64,
    /// The worst, in milliseconds.
    pub max_ms: f64,
}

/// A drive this device runs as the controller, for `lpctl input status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Driving {
    /// The driven device.
    pub peer: PublicKey,
    /// The edge of this screen the pointer left by, if it left by one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<Edge>,
    /// The driven screen's size, from `ready`.
    pub width: u32,
    /// The driven screen's height.
    pub height: u32,
    /// Batches sent so far.
    pub batches: u64,
}

/// One edge of the layout, for `lpctl input layout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutEntry {
    /// The edge of this screen.
    pub edge: Edge,
    /// The device it leads to.
    pub device: PublicKey,
}

/// What `lpctl input status` shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputStatus {
    /// What this device advertises.
    pub accepts: bool,
    /// What this device advertises.
    pub provides: bool,
    /// The drive this device runs, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driving: Option<Driving>,
    /// The device driving this one, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driven_by: Option<PublicKey>,
    /// Round-trip figures of the current or last drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round_trip: Option<RoundTrip>,
    /// The layout.
    pub layout: Vec<LayoutEntry>,
}

/// The controller side of one drive.
struct Drive {
    peer: PublicKey,
    edge: Option<Edge>,
    width: u32,
    height: u32,
    tx: mpsc::Sender<InputMessage>,
    batches: u64,
    held: HashSet<u16>,
    in_flight: HashMap<u64, Instant>,
    /// Set when a release or leave has already ended this drive, so the
    /// stream task does not release the capture twice.
    ended: bool,
}

/// The driven side of one drive.
struct Driven {
    peer: PublicKey,
    entry: Option<Edge>,
    /// Where the injected pointer is, as fractions.
    x: f64,
    y: f64,
    width: u32,
    height: u32,
    /// Keys and buttons held, so a stream that ends mid-press lifts them.
    held: Vec<InputEvent>,
}

/// The input synchroniser for one daemon.
pub struct InputSync {
    daemon: Arc<Daemon>,
    capture: Option<Arc<dyn Capture>>,
    inject: Option<Arc<dyn Inject>>,
    layout: Mutex<HashMap<Edge, PublicKey>>,
    drive: Mutex<Option<Drive>>,
    driven: Mutex<Option<Driven>>,
    samples: Mutex<Vec<f64>>,
    /// Per controller, the part of this screen its absolute positions
    /// refer to: the display it watches (milestone 8).
    regions: Mutex<HashMap<PublicKey, Region>>,
    offered: tokio::sync::Mutex<Option<InputParams>>,
    /// Local events, for the fakes and tests: a drive began or ended.
    changes: broadcast::Sender<Change>,
}

/// Something that happened to this device's drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// This device began driving `peer`.
    Driving(PublicKey),
    /// This device stopped driving `peer`.
    StoppedDriving(PublicKey),
    /// `peer` began driving this device.
    DrivenBy(PublicKey),
    /// `peer` stopped driving this device.
    Released(PublicKey),
}

impl std::fmt::Debug for InputSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InputSync")
    }
}

impl InputSync {
    /// Starts the input family for `daemon` with whichever sides the
    /// platform has: registers the stream handler, loads the layout, arms the
    /// capture's edges, watches it and advertises.
    #[must_use]
    pub fn start(
        daemon: Arc<Daemon>,
        capture: Option<Arc<dyn Capture>>,
        inject: Option<Arc<dyn Inject>>,
    ) -> Arc<Self> {
        let layout: HashMap<Edge, PublicKey> = daemon
            .store()
            .input_layout()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(edge, key)| edge.parse::<Edge>().ok().map(|e| (e, key)))
            .collect();
        let (changes, _) = broadcast::channel(64);
        let this = Arc::new(Self {
            daemon: daemon.clone(),
            capture,
            inject,
            layout: Mutex::new(layout),
            drive: Mutex::new(None),
            driven: Mutex::new(None),
            samples: Mutex::new(Vec::new()),
            regions: Mutex::new(HashMap::new()),
            offered: tokio::sync::Mutex::new(None),
            changes,
        });
        daemon.manager().on_stream(INPUT_TYPE, this.handler());
        this.arm();
        if this.capture.is_some() {
            let watcher = this.clone();
            tokio::spawn(async move { watcher.watch_capture().await });
        }
        let advertise = this.clone();
        tokio::spawn(async move {
            loop {
                advertise.advertise().await;
                tokio::time::sleep(ADVERTISE_POLL).await;
            }
        });
        this
    }

    /// Makes `peer`'s absolute positions refer to `region` of this screen,
    /// or to the whole screen with `None`: the screen family sets it while
    /// `peer` watches one display of several.
    pub fn set_region(&self, peer: &PublicKey, region: Option<Region>) {
        let mut regions = lock(&self.regions);
        match region {
            Some(r) => {
                regions.insert(*peer, r);
            }
            None => {
                regions.remove(peer);
            }
        }
    }

    /// Local drive changes, for tests and the app.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    /// What this device advertises right now.
    #[must_use]
    pub fn params(&self) -> InputParams {
        let mut params = self.inject.as_ref().map(|i| i.params()).unwrap_or_default();
        params.provides = self.capture.as_ref().is_some_and(|c| c.provides());
        if !params.accepts {
            params.absolute = false;
            params.text = false;
        }
        params
    }

    /// Re-reads the backends and tells peers if the descriptor changed.
    pub async fn advertise(&self) {
        let mut offered = self.offered.lock().await;
        let params = self.params();
        if *offered == Some(params) {
            return;
        }
        self.daemon.registry().withdraw(INPUT_TYPE).await;
        if params.offers_anything() {
            self.daemon.registry().offer(input_descriptor(params)).await;
        }
        *offered = Some(params);
        tracing::info!(
            accepts = params.accepts,
            provides = params.provides,
            "input capability advertised"
        );
    }

    /// The layout: which edge leads to which device.
    #[must_use]
    pub fn layout(&self) -> Vec<LayoutEntry> {
        let layout = lock(&self.layout);
        Edge::ALL
            .iter()
            .filter_map(|edge| {
                layout.get(edge).map(|device| LayoutEntry {
                    edge: *edge,
                    device: *device,
                })
            })
            .collect()
    }

    /// Makes `edge` lead to `device`, in the store and for the capture.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] when the row cannot be written.
    pub fn set_edge(&self, edge: Edge, device: PublicKey) -> Result<(), Error> {
        self.daemon.store().set_input_edge(edge.as_str(), &device)?;
        lock(&self.layout).insert(edge, device);
        self.arm();
        Ok(())
    }

    /// Makes `edge` lead nowhere. Returns whether it led somewhere.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] when the row cannot be removed.
    pub fn unset_edge(&self, edge: Edge) -> Result<bool, Error> {
        let was = self.daemon.store().unset_input_edge(edge.as_str())?;
        lock(&self.layout).remove(&edge);
        self.arm();
        Ok(was)
    }

    /// What `lpctl input status` shows.
    #[must_use]
    pub fn status(&self) -> InputStatus {
        let params = self.params();
        InputStatus {
            accepts: params.accepts,
            provides: params.provides,
            driving: lock(&self.drive).as_ref().map(|d| Driving {
                peer: d.peer,
                edge: d.edge,
                width: d.width,
                height: d.height,
                batches: d.batches,
            }),
            driven_by: lock(&self.driven).as_ref().map(|d| d.peer),
            round_trip: self.round_trip(),
            layout: self.layout(),
        }
    }

    /// The round-trip figures so far, if any batch was measured.
    #[must_use]
    pub fn round_trip(&self) -> Option<RoundTrip> {
        let samples = lock(&self.samples);
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.clone();
        sorted.sort_by(f64::total_cmp);
        Some(RoundTrip {
            samples: sorted.len(),
            median_ms: sorted[sorted.len() / 2],
            max_ms: *sorted.last().unwrap_or(&0.0),
        })
    }

    /// Whether this device is driving another right now.
    #[must_use]
    pub fn is_driving(&self) -> bool {
        lock(&self.drive).is_some()
    }

    /// The device driving this one right now, if any.
    #[must_use]
    pub fn driven_by(&self) -> Option<PublicKey> {
        lock(&self.driven).as_ref().map(|d| d.peer)
    }

    /// Whether this device is driven by another desktop that entered across
    /// an edge, rather than by a touchpad or a viewer.
    fn driven_by_edge(&self) -> bool {
        lock(&self.driven)
            .as_ref()
            .is_some_and(|d| d.entry.is_some())
    }

    /// Starts driving `peer` with `enter`, for a touchpad or a viewer that
    /// has no edge: the stream is opened, `ready` awaited, and events then go
    /// through [`InputSync::send`]. Returns the driven screen's size.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] when `peer` is not this person's device or
    /// does not accept what `enter` asks, [`Error::Transport`] when the
    /// stream cannot be opened or `ready` does not come, and [`Error::Io`]
    /// when a drive is already running.
    pub async fn drive(self: &Arc<Self>, peer: PublicKey, enter: Enter) -> Result<Ready, Error> {
        if self.is_driving() {
            return Err(Error::Io("already driving a device".to_owned()));
        }
        let params = self.peer_params(&peer)?;
        let name = || {
            self.daemon
                .device_display_name(&peer)
                .unwrap_or_else(|| peer.fingerprint())
        };
        if !params.accepts {
            return Err(Error::Refused(format!("{} does not accept input", name())));
        }
        if enter.is_absolute() == Some(true) && !params.absolute {
            return Err(Error::Refused(format!(
                "{} does not accept an absolute entry",
                name()
            )));
        }
        let conn = self
            .daemon
            .manager()
            .peer(&peer)
            .ok_or_else(|| Error::Transport(format!("{peer} is not connected")))?;
        let mut stream = open_on(&conn, INPUT_TYPE).await?;
        stream.send(&InputMessage::Enter(enter)).await?;
        let ready = match tokio::time::timeout(READY_TIMEOUT, stream.recv::<InputMessage>()).await {
            Ok(Ok(Some(InputMessage::Ready(ready)))) => ready,
            Ok(Ok(Some(other))) => {
                stream.reset(lp_core::frame::MALFORMED_CODE);
                return Err(Error::Transport(format!(
                    "{peer} answered enter with {other:?}"
                )));
            }
            Ok(Ok(None)) | Ok(Err(_)) => {
                return Err(Error::Transport(format!("{peer} refused the drive")));
            }
            Err(_) => {
                stream.reset(lp_core::frame::REFUSED_CODE);
                return Err(Error::Transport(format!("{peer} did not answer enter")));
            }
        };
        let (tx, rx) = mpsc::channel::<InputMessage>(64);
        *lock(&self.drive) = Some(Drive {
            peer,
            edge: enter.edge.map(Edge::opposite),
            width: ready.width,
            height: ready.height,
            tx,
            batches: 0,
            held: HashSet::new(),
            in_flight: HashMap::new(),
            ended: false,
        });
        lock(&self.samples).clear();
        let _ = self.changes.send(Change::Driving(peer));
        tracing::info!(%peer, width = ready.width, height = ready.height, "driving");
        let this = self.clone();
        tokio::spawn(async move { this.run_drive(stream, rx).await });
        Ok(ready)
    }

    /// Sends a batch of events to the device this one is driving, scaling
    /// motion from `from_width` pixels (this screen's width, or `None` for
    /// events already in the driven screen's pixels or in fractions).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when nothing is being driven.
    pub async fn send(
        &self,
        events: Vec<InputEvent>,
        from_width: Option<u32>,
    ) -> Result<(), Error> {
        let (tx, message, chord) = {
            let mut guard = lock(&self.drive);
            let Some(drive) = guard.as_mut() else {
                return Err(Error::Io("not driving a device".to_owned()));
            };
            let scale = from_width.map_or(1.0, |w| f64::from(drive.width) / f64::from(w.max(1)));
            let mut chord = false;
            let events: Vec<InputEvent> = events
                .into_iter()
                .take(MAX_BATCH)
                .map(|event| match event {
                    InputEvent::Motion { dx, dy } => InputEvent::Motion {
                        dx: dx * scale,
                        dy: dy * scale,
                    },
                    InputEvent::Key { code, pressed } => {
                        if pressed {
                            drive.held.insert(code);
                        } else {
                            drive.held.remove(&code);
                        }
                        if drive.held.contains(&KEY_LEFTCTRL) && drive.held.contains(&KEY_RIGHTCTRL)
                        {
                            chord = true;
                        }
                        InputEvent::Key { code, pressed }
                    }
                    InputEvent::Button { code, pressed } => {
                        if pressed {
                            drive.held.insert(code);
                        } else {
                            drive.held.remove(&code);
                        }
                        InputEvent::Button { code, pressed }
                    }
                    other => other,
                })
                .collect();
            drive.batches += 1;
            let seq = (drive.batches % MEASURE_EVERY == 1).then_some(drive.batches);
            if let Some(seq) = seq {
                drive.in_flight.insert(seq, Instant::now());
            }
            (
                drive.tx.clone(),
                InputMessage::Events(Events { events, seq }),
                chord,
            )
        };
        if tx.send(message).await.is_err() {
            return Err(Error::Transport("the drive's stream has gone".to_owned()));
        }
        if chord {
            tracing::info!("release chord: giving the input back");
            self.release_local(None).await;
        }
        Ok(())
    }

    /// Ends the drive this device runs, from the chord, `lpctl input release`
    /// or the capture's user leaving the touchpad: `release` goes to the
    /// driven device and the capture is given back.
    pub async fn release_local(&self, back_at: Option<(Edge, f64)>) {
        let (peer, tx) = {
            let mut guard = lock(&self.drive);
            let Some(drive) = guard.as_mut() else {
                return;
            };
            if drive.ended {
                return;
            }
            drive.ended = true;
            (drive.peer, drive.tx.clone())
        };
        let _ = tx.send(InputMessage::Release).await;
        self.end_drive(&peer, back_at);
    }

    /// A peer disconnected: whichever side of a drive it held ends.
    pub fn detach(&self, peer: &PublicKey) {
        let driving = lock(&self.drive).as_ref().is_some_and(|d| d.peer == *peer);
        if driving {
            tracing::info!(%peer, "the driven device went away; giving the input back");
            self.end_drive(peer, None);
        }
        let driven = lock(&self.driven).as_ref().is_some_and(|d| d.peer == *peer);
        if driven {
            tracing::info!(%peer, "the controller went away; lifting everything");
            self.end_driven(peer);
        }
    }

    /// Ends the controller side: the capture is released and the drive
    /// forgotten. Idempotent.
    fn end_drive(&self, peer: &PublicKey, back_at: Option<(Edge, f64)>) {
        let ended = {
            let mut guard = lock(&self.drive);
            match guard.as_ref() {
                Some(d) if d.peer == *peer => guard.take(),
                _ => None,
            }
        };
        if ended.is_none() {
            return;
        }
        if let Some(capture) = &self.capture {
            capture.release(back_at);
        }
        let _ = self.changes.send(Change::StoppedDriving(*peer));
        tracing::info!(%peer, "stopped driving");
    }

    /// Ends the driven side: everything held is lifted and the drive
    /// forgotten. Idempotent.
    fn end_driven(&self, peer: &PublicKey) {
        let ended = {
            let mut guard = lock(&self.driven);
            match guard.as_ref() {
                Some(d) if d.peer == *peer => guard.take(),
                _ => None,
            }
        };
        if ended.is_none() {
            return;
        }
        if let Some(inject) = &self.inject {
            inject.end();
        }
        let _ = self.changes.send(Change::Released(*peer));
        tracing::info!(%peer, "no longer driven");
    }

    /// Arms the capture with the edges the layout maps.
    fn arm(&self) {
        if let Some(capture) = &self.capture {
            let edges: Vec<Edge> = lock(&self.layout).keys().copied().collect();
            capture.arm(&edges);
        }
    }

    fn peer_params(&self, peer: &PublicKey) -> Result<InputParams, Error> {
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
                    .find_map(InputParams::from_descriptor)
            })
            .ok_or_else(|| Error::Chain(format!("{peer} offers no input")))
    }

    fn same_person(&self, peer: &PublicKey) -> bool {
        matches!(
            self.daemon.recognise(peer),
            Ok(Recognition::MyBinding { .. })
        )
    }

    /// The controller's stream task: sends what [`InputSync::send`] queues,
    /// reads `ready`'s successors, and ends the drive when the stream does.
    async fn run_drive(
        self: Arc<Self>,
        mut stream: FramedStream,
        mut rx: mpsc::Receiver<InputMessage>,
    ) {
        let peer = match lock(&self.drive).as_ref() {
            Some(d) => d.peer,
            None => return,
        };
        let mut back_at = None;
        loop {
            tokio::select! {
                outgoing = rx.recv() => {
                    match outgoing {
                        Some(message) => {
                            let release = matches!(message, InputMessage::Release);
                            if stream.send(&message).await.is_err() {
                                break;
                            }
                            if release {
                                stream.finish();
                                break;
                            }
                        }
                        None => break,
                    }
                }
                incoming = stream.recv::<InputMessage>() => {
                    match incoming {
                        Ok(Some(InputMessage::Injected(Injected { seq }))) => {
                            let sent = lock(&self.drive).as_mut().and_then(|d| d.in_flight.remove(&seq));
                            if let Some(sent) = sent {
                                let ms = sent.elapsed().as_secs_f64() * 1000.0;
                                let mut samples = lock(&self.samples);
                                if samples.len() >= SAMPLES {
                                    samples.remove(0);
                                }
                                samples.push(ms);
                            }
                        }
                        Ok(Some(InputMessage::Leave(leave))) => {
                            back_at = Some((leave.edge.opposite(), leave.at));
                            stream.finish();
                            break;
                        }
                        Ok(Some(InputMessage::Release)) => {
                            stream.finish();
                            break;
                        }
                        Ok(Some(other)) => {
                            tracing::debug!(%peer, ?other, "unexpected message from the driven device");
                        }
                        Ok(None) | Err(_) => break,
                    }
                }
            }
        }
        if let Some(d) = lock(&self.drive).as_mut() {
            d.ended = true;
        }
        self.end_drive(&peer, back_at);
    }

    /// The capture's events: an edge hit starts a drive, captured input is
    /// batched per frame and sent.
    async fn watch_capture(self: Arc<Self>) {
        let Some(capture) = self.capture.clone() else {
            return;
        };
        let mut events = capture.subscribe();
        let mut batch: Vec<InputEvent> = Vec::new();
        // Captured input arrived with nothing driven and the capture was
        // given back: the rest of that burst does not give it back again.
        let mut stray_released = false;
        let mut frame = tokio::time::interval(FRAME);
        frame.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                event = events.recv() => {
                    match event {
                        Ok(CaptureEvent::EdgeHit { edge, at }) => {
                            // A device driven across an edge never follows
                            // its own layout (decision 7): the pointer at its
                            // edge is the other desktop's, and following it
                            // could loop. A touchpad's or a viewer's drive
                            // (an absolute entry) is like a mouse plugged in,
                            // and the edge leads on. A compositor may
                            // have captured the input already (the
                            // InputCapture portal does when the pointer, the
                            // controller's included, crosses a barrier), so
                            // it is given back, or the desktop is locked out.
                            if self.is_driving() {
                                continue;
                            }
                            if self.driven_by_edge() {
                                tracing::info!(?edge, "the pointer reached an edge while driven; giving the input back");
                                capture.release(Some((edge, at)));
                                stray_released = true;
                                continue;
                            }
                            let Some(peer) = lock(&self.layout).get(&edge).copied() else {
                                continue;
                            };
                            stray_released = false;
                            match self.drive(peer, Enter::edge(edge.opposite(), at)).await {
                                Ok(_) => {
                                    if let Err(e) = capture.take(edge) {
                                        tracing::warn!(%e, "cannot take the input; releasing the drive");
                                        self.release_local(Some((edge, at))).await;
                                    }
                                }
                                Err(e) => {
                                    // A compositor may have captured the input
                                    // already when it reports the hit (the
                                    // InputCapture portal does): give it back,
                                    // or the desktop is locked out.
                                    tracing::info!(%peer, %e, "the pointer reached the edge but the drive did not start; giving the input back");
                                    capture.release(Some((edge, at)));
                                    stray_released = true;
                                }
                            }
                        }
                        Ok(CaptureEvent::Lost) => {
                            if self.is_driving() {
                                tracing::info!("the platform took the input back; ending the drive");
                                self.release_local(None).await;
                            }
                        }
                        Ok(CaptureEvent::Input(event)) => {
                            if !self.is_driving() {
                                // The platform holds the keyboard and mouse
                                // for no drive: give them back at once, so no
                                // path can leave the desktop without them.
                                if !stray_released {
                                    stray_released = true;
                                    tracing::warn!("captured input with nothing driven; giving it back");
                                    capture.release(None);
                                }
                                continue;
                            }
                            stray_released = false;
                            batch.push(event);
                            if batch.len() >= MAX_BATCH {
                                let width = capture.screen().0;
                                let _ = self.send(std::mem::take(&mut batch), Some(width)).await;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(n, "captured events lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
                _ = frame.tick() => {
                    if !batch.is_empty() {
                        let width = capture.screen().0;
                        let _ = self.send(std::mem::take(&mut batch), Some(width)).await;
                    }
                }
            }
        }
    }

    /// Handles one message from the controller on the driven side.
    fn apply(&self, peer: &PublicKey, message: InputMessage) -> Result<Vec<InputMessage>, Error> {
        if !self.same_person(peer) {
            return Err(Error::Chain(
                "input from a device that is not mine".to_owned(),
            ));
        }
        let Some(inject) = &self.inject else {
            return Err(Error::Chain("this device accepts no input".to_owned()));
        };
        let params = inject.params();
        match message {
            InputMessage::Enter(enter) => {
                if !params.accepts {
                    return Err(Error::Chain("this device accepts no input".to_owned()));
                }
                let absolute = enter.is_absolute().ok_or_else(|| {
                    Error::Decode("enter without an edge or a position".to_owned())
                })?;
                if absolute && !params.absolute {
                    return Err(Error::Chain(
                        "this device takes no absolute entry".to_owned(),
                    ));
                }
                if lock(&self.driven).is_some() {
                    return Err(Error::Io("already driven".to_owned()));
                }
                let region = lock(&self.regions)
                    .get(peer)
                    .copied()
                    .unwrap_or(Region::WHOLE);
                let start = enter.start().unwrap_or((0.5, 0.5));
                let start = if absolute {
                    region.map(start.0, start.1)
                } else {
                    start
                };
                let (width, height) = inject.screen();
                inject.begin(start)?;
                *lock(&self.driven) = Some(Driven {
                    peer: *peer,
                    entry: enter.edge,
                    x: start.0,
                    y: start.1,
                    width,
                    height,
                    held: Vec::new(),
                });
                let _ = self.changes.send(Change::DrivenBy(*peer));
                crate::grants::record_implicit_for(&self.daemon, peer, INPUT_TYPE);
                tracing::info!(%peer, "driven");
                Ok(vec![InputMessage::Ready(Ready { width, height })])
            }
            InputMessage::Events(batch) => {
                let mut out = Vec::new();
                let mut left: Option<Leave> = None;
                let region = lock(&self.regions)
                    .get(peer)
                    .copied()
                    .unwrap_or(Region::WHOLE);
                {
                    let mut guard = lock(&self.driven);
                    let Some(driven) = guard.as_mut().filter(|d| d.peer == *peer) else {
                        return Err(Error::Io("events without a drive".to_owned()));
                    };
                    for event in batch.events {
                        if left.is_some() {
                            break;
                        }
                        match &event {
                            InputEvent::Motion { dx, dy } => {
                                driven.x += dx / f64::from(driven.width.max(1));
                                driven.y += dy / f64::from(driven.height.max(1));
                                if let Some(edge) = driven.entry
                                    && crossed(edge, driven.x, driven.y)
                                {
                                    left = Some(Leave {
                                        edge,
                                        at: match edge {
                                            Edge::Left | Edge::Right => driven.y.clamp(0.0, 1.0),
                                            Edge::Top | Edge::Bottom => driven.x.clamp(0.0, 1.0),
                                        },
                                    });
                                    continue;
                                }
                                driven.x = driven.x.clamp(0.0, 1.0);
                                driven.y = driven.y.clamp(0.0, 1.0);
                            }
                            InputEvent::Position { x, y } => {
                                if !params.absolute {
                                    continue;
                                }
                                let (mx, my) = region.map(*x, *y);
                                driven.x = mx;
                                driven.y = my;
                                inject.inject(&InputEvent::Position { x: mx, y: my })?;
                                continue;
                            }
                            InputEvent::Text { .. } if !params.text => continue,
                            InputEvent::Key { code, pressed }
                            | InputEvent::Button { code, pressed } => {
                                let lifted = match &event {
                                    InputEvent::Key { .. } => InputEvent::Key {
                                        code: *code,
                                        pressed: false,
                                    },
                                    _ => InputEvent::Button {
                                        code: *code,
                                        pressed: false,
                                    },
                                };
                                if *pressed {
                                    if !driven.held.contains(&lifted) {
                                        driven.held.push(lifted);
                                    }
                                } else {
                                    driven.held.retain(|h| h != &lifted);
                                }
                            }
                            InputEvent::Scroll { .. } | InputEvent::Text { .. } => {}
                        }
                        inject.inject(&event)?;
                    }
                    if let Some(seq) = batch.seq
                        && left.is_none()
                    {
                        out.push(InputMessage::Injected(Injected { seq }));
                    }
                }
                if let Some(leave) = left {
                    self.end_driven(peer);
                    out.push(InputMessage::Leave(leave));
                }
                Ok(out)
            }
            InputMessage::Release => {
                self.end_driven(peer);
                Ok(Vec::new())
            }
            other => Err(Error::Decode(format!(
                "unexpected message from the controller: {other:?}"
            ))),
        }
    }

    fn handler(self: &Arc<Self>) -> StreamHandler {
        let sync = self.clone();
        Arc::new(move |conn, mut stream| {
            let sync = sync.clone();
            Box::pin(async move {
                let peer = *conn.peer();
                loop {
                    match stream.recv::<InputMessage>().await {
                        Ok(Some(message)) => match sync.apply(&peer, message) {
                            Ok(replies) => {
                                let mut ending = false;
                                for reply in replies {
                                    ending |= matches!(reply, InputMessage::Leave(_));
                                    if stream.send(&reply).await.is_err() {
                                        ending = true;
                                        break;
                                    }
                                }
                                if ending || sync.driven_by() != Some(peer) {
                                    stream.finish();
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(%peer, %e, "input message refused");
                                stream.reset(lp_core::frame::REFUSED_CODE);
                                break;
                            }
                        },
                        Ok(None) => break,
                        Err(e) => {
                            tracing::debug!(%peer, %e, "input stream ended");
                            break;
                        }
                    }
                }
                // However the stream ended, nothing stays pressed.
                sync.end_driven(&peer);
            })
        })
    }
}

/// Whether a pointer at (`x`, `y`) has gone beyond `edge`.
fn crossed(edge: Edge, x: f64, y: f64) -> bool {
    match edge {
        Edge::Left => x < 0.0,
        Edge::Right => x > 1.0,
        Edge::Top => y < 0.0,
        Edge::Bottom => y > 1.0,
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A capture with no edges, for a device whose app drives through
/// [`InputSync::drive`] and [`InputSync::send`]: the phone as a touchpad. It
/// advertises `provides` and never reports an event of its own.
#[derive(Debug, Default)]
pub struct Touchpad;

impl Touchpad {
    /// The phone's capture.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl Capture for Touchpad {
    fn provides(&self) -> bool {
        true
    }

    fn screen(&self) -> (u32, u32) {
        (0, 0)
    }

    fn arm(&self, _edges: &[Edge]) {}

    fn take(&self, _edge: Edge) -> Result<(), Error> {
        Ok(())
    }

    fn release(&self, _back_at: Option<(Edge, f64)>) {}

    fn subscribe(&self) -> broadcast::Receiver<CaptureEvent> {
        let (tx, rx) = broadcast::channel(1);
        // Nothing ever sends; the receiver lives as long as it is held.
        std::mem::forget(tx);
        rx
    }
}

/// An in-memory capture for tests and `LIGATURE_FAKE_INPUT=1`: the test
/// hits an edge and feeds events; the fake records what was taken and where
/// the pointer was put back.
pub struct FakeCapture {
    provides: Mutex<bool>,
    screen: (u32, u32),
    armed: Mutex<Vec<Edge>>,
    taken: Mutex<Option<Edge>>,
    releases: Mutex<Vec<Option<(Edge, f64)>>>,
    events: broadcast::Sender<CaptureEvent>,
}

impl FakeCapture {
    /// A capture of a screen `width` by `height` that provides.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(Self {
            provides: Mutex::new(true),
            screen: (width, height),
            armed: Mutex::new(Vec::new()),
            taken: Mutex::new(None),
            releases: Mutex::new(Vec::new()),
            events,
        })
    }

    /// Switches capture on or off, as a platform permission would.
    pub fn set_provides(&self, provides: bool) {
        *lock(&self.provides) = provides;
    }

    /// The edges the sync armed, sorted.
    #[must_use]
    pub fn armed(&self) -> Vec<Edge> {
        let mut edges = lock(&self.armed).clone();
        edges.sort_by_key(|e| e.as_str());
        edges
    }

    /// The edge taken right now, if captured.
    #[must_use]
    pub fn taken(&self) -> Option<Edge> {
        *lock(&self.taken)
    }

    /// Every release so far, with where the pointer was put back.
    #[must_use]
    pub fn releases(&self) -> Vec<Option<(Edge, f64)>> {
        lock(&self.releases).clone()
    }

    /// The pointer reaches `edge` at `at`.
    pub fn hit_edge(&self, edge: Edge, at: f64) {
        let _ = self.events.send(CaptureEvent::EdgeHit { edge, at });
    }

    /// The user does something while captured.
    pub fn feed(&self, event: InputEvent) {
        let _ = self.events.send(CaptureEvent::Input(event));
    }
}

impl Capture for FakeCapture {
    fn provides(&self) -> bool {
        *lock(&self.provides)
    }

    fn screen(&self) -> (u32, u32) {
        self.screen
    }

    fn arm(&self, edges: &[Edge]) {
        *lock(&self.armed) = edges.to_vec();
    }

    fn take(&self, edge: Edge) -> Result<(), Error> {
        *lock(&self.taken) = Some(edge);
        Ok(())
    }

    fn release(&self, back_at: Option<(Edge, f64)>) {
        *lock(&self.taken) = None;
        lock(&self.releases).push(back_at);
    }

    fn subscribe(&self) -> broadcast::Receiver<CaptureEvent> {
        self.events.subscribe()
    }
}

/// An in-memory injector for tests and `LIGATURE_FAKE_INPUT=1`: records
/// where drives began, every event injected and every end.
pub struct FakeInject {
    params: Mutex<InputParams>,
    screen: (u32, u32),
    begun: Mutex<Vec<(f64, f64)>>,
    injected: Mutex<Vec<InputEvent>>,
    ends: Mutex<usize>,
}

impl FakeInject {
    /// An injector of a screen `width` by `height` that accepts everything.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Arc<Self> {
        Arc::new(Self {
            params: Mutex::new(InputParams {
                accepts: true,
                provides: false,
                absolute: true,
                text: true,
            }),
            screen: (width, height),
            begun: Mutex::new(Vec::new()),
            injected: Mutex::new(Vec::new()),
            ends: Mutex::new(0),
        })
    }

    /// Changes what the injector takes, as a platform permission would.
    pub fn set_params(&self, params: InputParams) {
        *lock(&self.params) = params;
    }

    /// Where drives began, as fractions.
    #[must_use]
    pub fn begun(&self) -> Vec<(f64, f64)> {
        lock(&self.begun).clone()
    }

    /// Every event injected so far, in order.
    #[must_use]
    pub fn injected(&self) -> Vec<InputEvent> {
        lock(&self.injected).clone()
    }

    /// Forgets the events injected so far.
    pub fn clear(&self) {
        lock(&self.injected).clear();
    }

    /// How many times a drive ended, lifting everything.
    #[must_use]
    pub fn ends(&self) -> usize {
        *lock(&self.ends)
    }
}

impl Inject for FakeInject {
    fn params(&self) -> InputParams {
        *lock(&self.params)
    }

    fn screen(&self) -> (u32, u32) {
        self.screen
    }

    fn begin(&self, start: (f64, f64)) -> Result<(), Error> {
        lock(&self.begun).push(start);
        Ok(())
    }

    fn inject(&self, event: &InputEvent) -> Result<(), Error> {
        lock(&self.injected).push(event.clone());
        Ok(())
    }

    fn end(&self) {
        *lock(&self.ends) += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_an_edge() {
        assert!(crossed(Edge::Left, -0.01, 0.5));
        assert!(!crossed(Edge::Left, 0.0, 0.5));
        assert!(crossed(Edge::Right, 1.01, 0.5));
        assert!(crossed(Edge::Top, 0.5, -0.1));
        assert!(crossed(Edge::Bottom, 0.5, 1.1));
        assert!(!crossed(Edge::Bottom, 0.5, 1.0));
    }
}
