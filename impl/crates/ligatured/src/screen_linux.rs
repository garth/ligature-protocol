//! The screen capability on Linux under a Wayland compositor (milestone 8,
//! LP-149): capture through the ScreenCast portal, or `wlr-screencopy` where
//! the portal is missing, and H.264 encoding by ffmpeg.
//!
//! Nothing here links a media library. A share is a pipeline of child
//! processes whose bytes the daemon reads: on the portal path GStreamer's
//! `pipewiresrc` turns the portal's PipeWire stream into raw frames at the
//! agreed size and rate, and on the screencopy path the daemon's own helper
//! (`ligatured screen-helper capture`) copies an output at that rate; either
//! way the raw frames go into `ffmpeg`, which encodes H.264 with VA-API where
//! it works and `libx264` otherwise, an access unit delimiter before every
//! frame, the parameter sets before every keyframe and a keyframe every
//! second. The daemon splits the byte stream at the delimiters into frames.
//! A machine without ffmpeg's H.264 encoder does not advertise sharing, and
//! one without the portal and GStreamer's PipeWire element falls back to
//! screencopy.
//!
//! The portal shows its picker the first time and hands back a restore
//! token, kept in the store, so the next share of the same choice asks
//! nothing. Since the compositor shows no indicator of its own, a share posts
//! a local notification, "Sharing `display` with `device`", with a Stop
//! action, for its life.

use std::collections::HashMap;
use std::io::Write as _;
use std::os::fd::{AsRawFd as _, OwnedFd};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_core::Error;
use lp_core::frame;
use lp_core::input::Region;
use lp_core::screen::{Codec, Display, RefuseReason, ScreenParams, StopReason};
use lp_identity::trust_store::TrustStore;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::clipboard_linux::select_wayland_socket;
#[cfg(test)]
use crate::screen::encode::bit_rate;
#[cfg(test)]
use crate::screen::encode::{AccessUnits, KEYFRAME_EVERY_SECOND, X264_CRF, is_keyframe};
use crate::screen::encode::{Encoder, RawInput, encoder_args, probe_encoder, read_frames};
use crate::screen::{Capture, Captured, Settings, Share, ShareControl};

/// The store key of the portal's restore token.
pub const RESTORE_TOKEN_KEY: &str = "screen.restore_token";
/// Set to `screencopy` to capture through `wlr-screencopy` even where the
/// portal exists (tests in a nested compositor, a compositor whose portal
/// misbehaves).
pub const CAPTURE_ENV: &str = "LIGATURE_SCREEN_CAPTURE";
/// How often the displays and the tools are looked at again.
const PROBE_EVERY: Duration = Duration::from_secs(5);
/// The screen-cast portal is probed again every so many rounds while it is
/// missing.
const PORTAL_REPROBE_ROUNDS: u32 = 12;
/// The largest frame between the daemon and its screen helper's listing.
const HELPER_MAX_FRAME: usize = 64 * 1024;
/// How often `pipewiresrc` sends the last picture again while the screen is
/// still, in milliseconds: once a second, so a still screen costs the
/// viewer a picture a second rather than the share's full rate, and the
/// watchdog still sees the capture alive.
const KEEPALIVE_MS: u32 = 1000;
/// A share whose encoder has written nothing for this long has stalled: a
/// still screen still produces a frame a second ([`KEEPALIVE_MS`]).
const STALL: Duration = Duration::from_secs(4);
/// How often a share's watchdog looks.
const WATCH_EVERY: Duration = Duration::from_secs(1);
/// Restarts of a stalled capture before the share ends with an error.
const MAX_RESTARTS: u32 = 3;
/// A capture that has run this long since its last restart starts its count
/// of restarts again.
const RESTARTS_FORGIVEN: Duration = Duration::from_secs(60);

/// What a share's watchdog does about how long its encoder has been quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stall {
    /// Frames are coming.
    Fine,
    /// Start the capture again.
    Restart,
    /// Restarts did not hold: end the share.
    GiveUp,
}

/// The watchdog's count of restarts.
#[derive(Debug, Default)]
struct StallWatch {
    restarts: u32,
    restarted: Option<Instant>,
}

impl StallWatch {
    /// The verdict at `now` for an encoder quiet for `quiet`.
    fn check(&mut self, now: Instant, quiet: Duration) -> Stall {
        if quiet < STALL {
            if self
                .restarted
                .is_some_and(|at| now.duration_since(at) >= RESTARTS_FORGIVEN)
            {
                self.restarts = 0;
                self.restarted = None;
            }
            return Stall::Fine;
        }
        if self.restarts >= MAX_RESTARTS {
            return Stall::GiveUp;
        }
        self.restarts += 1;
        self.restarted = Some(now);
        Stall::Restart
    }
}

/// One output as the helper lists it: the display, and where it lies in the
/// layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputInfo {
    /// The output's name, which is the display's id.
    pub id: String,
    /// What the compositor describes it as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Logical left.
    pub x: i32,
    /// Logical top.
    pub y: i32,
    /// Logical width.
    pub width: u32,
    /// Logical height.
    pub height: u32,
}

/// What `screen-helper list` prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HelperList {
    /// The outputs.
    pub outputs: Vec<OutputInfo>,
    /// Whether the compositor offers `wlr-screencopy`.
    pub screencopy: bool,
}

impl HelperList {
    /// The outputs as displays: the one at the layout's origin is primary,
    /// or the first when none is.
    #[must_use]
    pub fn displays(&self) -> Vec<Display> {
        let primary = self
            .outputs
            .iter()
            .position(|o| o.x == 0 && o.y == 0)
            .unwrap_or(0);
        self.outputs
            .iter()
            .enumerate()
            .map(|(i, o)| Display {
                id: o.id.clone(),
                name: o.description.clone(),
                width: o.width,
                height: o.height,
                primary: i == primary,
            })
            .collect()
    }

    /// Where `id` lies in the layout, as fractions of its bounding box.
    #[must_use]
    pub fn region(&self, id: &str) -> Option<Region> {
        region_of(&self.outputs, |o| o.id == id)
    }

    /// The output at a logical position, as the portal reports a stream.
    #[must_use]
    pub fn at(&self, x: i32, y: i32) -> Option<&OutputInfo> {
        self.outputs.iter().find(|o| o.x == x && o.y == y)
    }
}

fn region_of(outputs: &[OutputInfo], pick: impl Fn(&OutputInfo) -> bool) -> Option<Region> {
    let o = outputs.iter().find(|o| pick(o))?;
    let left = outputs.iter().map(|o| o.x).min()?;
    let top = outputs.iter().map(|o| o.y).min()?;
    let right = outputs.iter().map(|o| o.x + o.width as i32).max()?;
    let bottom = outputs.iter().map(|o| o.y + o.height as i32).max()?;
    let (w, h) = (f64::from(right - left), f64::from(bottom - top));
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(Region {
        x: f64::from(o.x - left) / w,
        y: f64::from(o.y - top) / h,
        width: f64::from(o.width) / w,
        height: f64::from(o.height) / h,
    })
}

/// The GStreamer pipeline that turns the portal's PipeWire stream into raw
/// BGRx frames of the share's size on stdout. The compositor sends a frame
/// when the screen changes; `videorate` drops any beyond the share's rate
/// and never adds one, and `keepalive-time` sends the last again once a
/// second while the screen is still.
#[must_use]
pub fn pipewire_args(fd: i32, node: u32, settings: &Settings) -> Vec<String> {
    format!(
        "-q pipewiresrc fd={fd} path={node} do-timestamp=true keepalive-time={KEEPALIVE_MS} \
         ! videorate drop-only=true max-rate={} \
         ! videoconvert ! videoscale \
         ! video/x-raw,format=BGRx,width={},height={} \
         ! fdsink fd=1 sync=false",
        settings.frame_rate.max(1),
        settings.width,
        settings.height
    )
    .split_whitespace()
    .map(str::to_owned)
    .collect()
}

// ---------------------------------------------------------------------------
// The tools and the displays, looked at in the background.

#[derive(Debug, Default, Clone)]
struct Probe {
    encoder: Option<Encoder>,
    decoder: bool,
    pipewiresrc: bool,
    portal: bool,
    list: HelperList,
}

pub use crate::screen::{probe_decoder, viewer_params};

// ---------------------------------------------------------------------------
// The capture.

/// A running share's processes and portal session.
struct Running {
    children: Vec<Child>,
    portal: Option<Arc<PortalSession>>,
    /// Set when the current processes are stopped on purpose, so the end of
    /// their stream is not taken for a failure.
    generation: Arc<AtomicBool>,
}

/// Where a running share's raw frames come from, to start them again.
enum Source {
    /// The portal's stream: a new remote on the same session, no picker.
    Portal {
        session: Arc<PortalSession>,
        node: u32,
    },
    /// The screencopy helper.
    Screencopy,
}

/// A reader that notes when it last read anything: the share's watchdog
/// reads it.
struct Noting<R> {
    inner: R,
    last: Arc<Mutex<Instant>>,
}

impl<R: std::io::Read> std::io::Read for Noting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            *lock(&self.last) = Instant::now();
        }
        Ok(n)
    }
}

/// Starts the encoder on a producer's raw frames; returns it and its output.
fn spawn_encoder(
    encoder: Encoder,
    input: &RawInput,
    settings: &Settings,
    producer: &mut Child,
) -> Result<(Child, ChildStdout), RefuseReason> {
    let raw = producer.stdout.take().ok_or(RefuseReason::NoEncoder)?;
    let mut ffmpeg = Command::new("ffmpeg")
        .args(encoder_args(encoder, input, settings))
        .stdin(Stdio::from(raw))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| {
            tracing::warn!(%e, "cannot start ffmpeg");
            let _ = producer.kill();
            RefuseReason::NoEncoder
        })?;
    let encoded = ffmpeg.stdout.take().ok_or(RefuseReason::NoEncoder)?;
    Ok((ffmpeg, encoded))
}

/// The raw frames `gst-launch` writes: the share's size, BGRx.
fn portal_input(settings: &Settings) -> RawInput {
    RawInput {
        width: settings.width,
        height: settings.height,
        pix_fmt: "bgr0".to_owned(),
    }
}

/// What the portal handed back for a share.
struct PortalStart {
    session: PortalSession,
    node: u32,
    position: Option<(i32, i32)>,
    fd: OwnedFd,
}

/// Where a share's raw frames come from.
struct Producer {
    child: Child,
    input: RawInput,
    portal: Option<(PortalSession, u32)>,
    region: Option<Region>,
}

struct PortalSession {
    /// The proxy the session was made through, kept for the session's life
    /// and for a new remote when the capture restarts.
    proxy: ashpd::desktop::screencast::Screencast,
    session: ashpd::desktop::Session<ashpd::desktop::screencast::Screencast>,
}

/// The Linux screen capture. Implements [`Capture`].
#[derive(Clone)]
pub struct LinuxScreen {
    handle: tokio::runtime::Handle,
    store: Arc<TrustStore>,
    probe: Arc<Mutex<Probe>>,
    runtime_dir: PathBuf,
    env_display: Option<std::ffi::OsString>,
    helper_exe: PathBuf,
    force_screencopy: bool,
}

impl std::fmt::Debug for LinuxScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LinuxScreen")
    }
}

impl LinuxScreen {
    /// Starts the capture: probes the tools and lists the displays in the
    /// background. Called within the daemon's runtime.
    #[must_use]
    pub fn start(store: Arc<TrustStore>) -> Arc<Self> {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let force = std::env::var(CAPTURE_ENV).is_ok_and(|v| v == "screencopy");
        Self::start_with(
            store,
            runtime_dir,
            std::env::var_os("WAYLAND_DISPLAY"),
            None,
            force,
        )
    }

    /// As [`LinuxScreen::start`], with the socket selection's inputs, the
    /// binary providing the helper, and whether to skip the portal.
    #[must_use]
    pub fn start_with(
        store: Arc<TrustStore>,
        runtime_dir: PathBuf,
        env_display: Option<std::ffi::OsString>,
        helper_exe: Option<PathBuf>,
        force_screencopy: bool,
    ) -> Arc<Self> {
        let helper_exe = helper_exe
            .or_else(|| std::env::current_exe().ok())
            .unwrap_or_else(|| PathBuf::from("ligatured"));
        let this = Arc::new(Self {
            handle: tokio::runtime::Handle::current(),
            store,
            probe: Arc::new(Mutex::new(Probe::default())),
            runtime_dir,
            env_display,
            helper_exe,
            force_screencopy,
        });
        let prober = this.clone();
        std::thread::Builder::new()
            .name("screen-probe".into())
            .spawn(move || prober.probe_loop())
            .expect("spawn the screen probe");
        this
    }

    /// Lists the outputs now, blocking until the helper answers.
    fn list_now(&self) -> HelperList {
        let Some(socket) = select_wayland_socket(self.env_display.as_deref(), &self.runtime_dir)
        else {
            return HelperList::default();
        };
        let out = Command::new(&self.helper_exe)
            .args(["screen-helper", "list"])
            .env("WAYLAND_DISPLAY", &socket)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        match out {
            Ok(out) if out.status.success() => read_list(&out.stdout).unwrap_or_default(),
            _ => HelperList::default(),
        }
    }

    fn probe_loop(self: Arc<Self>) {
        let encoder = probe_encoder();
        let decoder = probe_decoder();
        let pipewiresrc = Command::new("gst-inspect-1.0")
            .args(["--exists", "pipewiresrc"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        let probe_portal = || {
            !self.force_screencopy
                && self
                    .handle
                    .block_on(async {
                        // Not before the session: an early call would start
                        // the portal without its environment (LP-157).
                        crate::portal_gate::wait_for_session().await;
                        let proxy = ashpd::desktop::screencast::Screencast::new().await?;
                        proxy.available_source_types().await
                    })
                    .is_ok_and(|types| {
                        types.contains(ashpd::desktop::screencast::SourceType::Monitor)
                    })
        };
        let mut portal = probe_portal();
        tracing::info!(
            ?encoder,
            decoder,
            pipewiresrc,
            portal,
            "screen sharing tools probed"
        );
        let mut round: u32 = 0;
        loop {
            // A portal that was missing (or started without the session) is
            // looked at again every minute, so a repaired one is found.
            round = round.wrapping_add(1);
            if !portal && !self.force_screencopy && round.is_multiple_of(PORTAL_REPROBE_ROUNDS) {
                portal = probe_portal();
                if portal {
                    tracing::info!("the screen-cast portal is available now");
                }
            }
            let list = self.list_now();
            {
                let mut probe = lock(&self.probe);
                probe.encoder = encoder;
                probe.decoder = decoder;
                probe.pipewiresrc = pipewiresrc;
                probe.portal = portal;
                if probe.list != list {
                    tracing::debug!(
                        outputs = list.outputs.len(),
                        screencopy = list.screencopy,
                        "displays changed"
                    );
                }
                probe.list = list;
            }
            std::thread::sleep(PROBE_EVERY);
        }
    }

    fn use_portal(&self, probe: &Probe) -> bool {
        !self.force_screencopy && probe.portal && probe.pipewiresrc
    }

    /// The portal's choice: a session, the stream's node, the remote's fd and
    /// the output it is. Blocks on the picker the first time.
    fn open_portal(&self) -> Result<PortalStart, RefuseReason> {
        use ashpd::desktop::screencast::{
            CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
            StartCastOptions,
        };
        use ashpd::desktop::{CreateSessionOptions, PersistMode};
        let token = self.store.local_setting(RESTORE_TOKEN_KEY).ok().flatten();
        let store = self.store.clone();
        let result = self.handle.block_on(async move {
            let proxy = Screencast::new().await?;
            let session = proxy
                .create_session(CreateSessionOptions::default())
                .await?;
            proxy
                .select_sources(
                    &session,
                    SelectSourcesOptions::default()
                        .set_cursor_mode(CursorMode::Embedded)
                        .set_sources(ashpd::enumflags2::BitFlags::from_flag(SourceType::Monitor))
                        .set_multiple(false)
                        .set_restore_token(token.as_deref())
                        .set_persist_mode(PersistMode::ExplicitlyRevoked),
                )
                .await?
                .response()?;
            let streams = proxy
                .start(&session, None, StartCastOptions::default())
                .await?
                .response()?;
            if let Some(t) = streams.restore_token()
                && let Err(e) = store.set_local_setting(RESTORE_TOKEN_KEY, Some(t))
            {
                tracing::warn!(%e, "cannot keep the screen-cast restore token");
            }
            let stream = streams.streams().first().ok_or(ashpd::Error::NoResponse)?;
            let (node, position) = (stream.pipe_wire_node_id(), stream.position());
            let fd = proxy
                .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
                .await?;
            Ok::<_, ashpd::Error>(PortalStart {
                session: PortalSession { proxy, session },
                node,
                position,
                fd,
            })
        });
        result.map_err(|e| {
            tracing::info!(%e, "the screen-cast portal did not start a share");
            RefuseReason::Consent
        })
    }

    fn producer_portal(&self, settings: &Settings) -> Result<Producer, RefuseReason> {
        let PortalStart {
            session: portal,
            node,
            position,
            fd,
        } = self.open_portal()?;
        let list = lock(&self.probe).list.clone();
        let region = position
            .and_then(|(x, y)| list.at(x, y).map(|o| o.id.clone()))
            .and_then(|id| list.region(&id));
        let child = Self::gst(fd, node, settings)?;
        Ok(Producer {
            child,
            input: portal_input(settings),
            portal: Some((portal, node)),
            region,
        })
    }

    /// `gst-launch-1.0` reading the portal's stream `node` through the
    /// PipeWire remote `fd`.
    fn gst(fd: OwnedFd, node: u32, settings: &Settings) -> Result<Child, RefuseReason> {
        // The child inherits a duplicate without close-on-exec, by number.
        let inherited = rustix::io::dup(&fd).map_err(|e| {
            tracing::warn!(%e, "cannot pass the PipeWire remote to GStreamer");
            RefuseReason::NoEncoder
        })?;
        let child = Command::new("gst-launch-1.0")
            .args(pipewire_args(inherited.as_raw_fd(), node, settings))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                tracing::warn!(%e, "cannot start gst-launch-1.0");
                RefuseReason::NoEncoder
            })?;
        drop(inherited);
        drop(fd);
        Ok(child)
    }

    /// Starts a share's processes again from its source: a new producer and
    /// a new encoder, whose first frame is a keyframe. Blocks; called off the
    /// runtime.
    fn relaunch(
        &self,
        source: &Source,
        encoder: Encoder,
        settings: &Settings,
    ) -> Result<(Vec<Child>, ChildStdout), RefuseReason> {
        let (mut producer, input) = match source {
            Source::Portal { session, node } => {
                use ashpd::desktop::screencast::OpenPipeWireRemoteOptions;
                let fd = self
                    .handle
                    .block_on(session.proxy.open_pipe_wire_remote(
                        &session.session,
                        OpenPipeWireRemoteOptions::default(),
                    ))
                    .map_err(|e| {
                        tracing::warn!(%e, "cannot open the screen-cast stream again");
                        RefuseReason::NoEncoder
                    })?;
                (Self::gst(fd, *node, settings)?, portal_input(settings))
            }
            Source::Screencopy => {
                let p = self.producer_screencopy(settings)?;
                (p.child, p.input)
            }
        };
        let (ffmpeg, encoded) = spawn_encoder(encoder, &input, settings, &mut producer)?;
        Ok((vec![producer, ffmpeg], encoded))
    }

    /// The share's watchdog: every second, restarts a capture whose encoder
    /// has written nothing for [`STALL`], and ends the share once restarts
    /// do not hold. Runs on a thread of its own until the share stops.
    #[allow(clippy::too_many_arguments)]
    fn watch_capture(
        &self,
        source: &Source,
        encoder: Encoder,
        settings: &Settings,
        running: &Mutex<Running>,
        stopping: &AtomicBool,
        frames_tx: &mpsc::UnboundedSender<Captured>,
        last: &Arc<Mutex<Instant>>,
    ) {
        let mut watch = StallWatch::default();
        loop {
            std::thread::sleep(WATCH_EVERY);
            if stopping.load(Ordering::SeqCst) || frames_tx.is_closed() {
                return;
            }
            let quiet = lock(last).elapsed();
            match watch.check(Instant::now(), quiet) {
                Stall::Fine => continue,
                Stall::GiveUp => {
                    tracing::warn!("the screen capture keeps stalling; ending the share");
                    let _ = frames_tx.send(Captured::Ended(StopReason::Error));
                    return;
                }
                Stall::Restart => {}
            }
            tracing::warn!(
                quiet_ms = quiet.as_millis(),
                restarts = watch.restarts,
                "the screen capture stalled; starting it again"
            );
            let old = {
                let mut r = lock(running);
                r.generation.store(true, Ordering::SeqCst);
                std::mem::take(&mut r.children)
            };
            for mut child in old {
                let _ = child.kill();
                let _ = child.wait();
            }
            let Ok((mut children, encoded)) = self.relaunch(source, encoder, settings) else {
                let _ = frames_tx.send(Captured::Ended(StopReason::Error));
                return;
            };
            let generation = Arc::new(AtomicBool::new(false));
            {
                let mut r = lock(running);
                if stopping.load(Ordering::SeqCst) {
                    for child in &mut children {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    return;
                }
                r.children = children;
                r.generation = generation.clone();
            }
            *lock(last) = Instant::now();
            let noting = Noting {
                inner: encoded,
                last: last.clone(),
            };
            if read_frames(noting, frames_tx.clone(), generation).is_err() {
                let _ = frames_tx.send(Captured::Ended(StopReason::Error));
                return;
            }
        }
    }

    fn producer_screencopy(&self, settings: &Settings) -> Result<Producer, RefuseReason> {
        let socket = select_wayland_socket(self.env_display.as_deref(), &self.runtime_dir)
            .ok_or(RefuseReason::NoDisplay)?;
        let mut child = Command::new(&self.helper_exe)
            .args([
                "screen-helper",
                "capture",
                &settings.display,
                &settings.frame_rate.to_string(),
            ])
            .env("WAYLAND_DISPLAY", &socket)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                tracing::warn!(%e, "cannot start the screen helper");
                RefuseReason::NoDisplay
            })?;
        let stdout = child.stdout.as_mut().ok_or(RefuseReason::NoDisplay)?;
        let input = read_raw_header(stdout).map_err(|e| {
            tracing::warn!(%e, "the screen helper did not start capturing");
            let _ = child.kill();
            RefuseReason::NoDisplay
        })?;
        let region = lock(&self.probe).list.region(&settings.display);
        Ok(Producer {
            child,
            input,
            portal: None,
            region,
        })
    }
}

/// Reads `LPRAW <width> <height> <pix_fmt>\n`, the helper's first line.
fn read_raw_header<R: std::io::Read>(r: &mut R) -> Result<RawInput, Error> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while line.len() < 64 {
        r.read_exact(&mut byte)
            .map_err(|e| Error::Io(format!("screen helper: {e}")))?;
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
    }
    let line = String::from_utf8(line).map_err(|e| Error::Decode(e.to_string()))?;
    let mut parts = line.split_whitespace();
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("LPRAW"), Some(w), Some(h), Some(fmt)) => Ok(RawInput {
            width: w.parse().map_err(|_| Error::Decode(line.clone()))?,
            height: h.parse().map_err(|_| Error::Decode(line.clone()))?,
            pix_fmt: fmt.to_owned(),
        }),
        _ => Err(Error::Decode(format!("screen helper header: {line}"))),
    }
}

fn read_list(body: &[u8]) -> Result<HelperList, Error> {
    let len = body.len();
    if len < 4 {
        return Err(Error::Decode("short helper listing".to_owned()));
    }
    let n = frame::body_len([body[0], body[1], body[2], body[3]], HELPER_MAX_FRAME)?;
    frame::decode(
        body.get(4..4 + n)
            .ok_or_else(|| Error::Decode("truncated listing".to_owned()))?,
    )
}

impl Capture for LinuxScreen {
    fn params(&self) -> ScreenParams {
        let probe = lock(&self.probe);
        let can_capture =
            !probe.list.outputs.is_empty() && (self.use_portal(&probe) || probe.list.screencopy);
        ScreenParams {
            shares: probe.encoder.is_some() && can_capture,
            views: false,
            codecs: vec![Codec::H264],
            ..ScreenParams::default()
        }
    }

    fn displays(&self) -> Vec<Display> {
        lock(&self.probe).list.displays()
    }

    fn region(&self, display: &str) -> Option<Region> {
        lock(&self.probe).list.region(display)
    }

    fn start(&self, settings: &Settings) -> Result<Share, RefuseReason> {
        let probe = lock(&self.probe).clone();
        let encoder = probe.encoder.ok_or(RefuseReason::NoEncoder)?;
        let Producer {
            child: mut producer,
            input,
            portal,
            region,
        } = if self.use_portal(&probe) {
            self.producer_portal(settings)?
        } else {
            self.producer_screencopy(settings)?
        };
        let (ffmpeg, encoded) = spawn_encoder(encoder, &input, settings, &mut producer)?;
        tracing::info!(
            display = %settings.display,
            ?encoder,
            portal = portal.is_some(),
            input = format!("{}x{} {}", input.width, input.height, input.pix_fmt),
            "screen capture started"
        );

        let (frames_tx, frames) = mpsc::unbounded_channel::<Captured>();
        let (control, mut control_rx) = mpsc::unbounded_channel::<ShareControl>();
        let stopping = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicBool::new(false));
        let (portal, source) = match portal {
            Some((session, node)) => {
                let session = Arc::new(session);
                (Some(session.clone()), Source::Portal { session, node })
            }
            None => (None, Source::Screencopy),
        };
        let running = Arc::new(Mutex::new(Running {
            children: vec![producer, ffmpeg],
            portal,
            generation: generation.clone(),
        }));

        // The encoded stream, split into frames, and when it last moved.
        let last = Arc::new(Mutex::new(Instant::now()));
        let noting = Noting {
            inner: encoded,
            last: last.clone(),
        };
        read_frames(noting, frames_tx.clone(), generation).map_err(|_| RefuseReason::NoEncoder)?;

        // A capture that produces nothing has stalled, since `keepalive-time`
        // keeps a still screen producing frames: it is started again.
        {
            let this = self.clone();
            let (running, stopping, frames_tx) =
                (running.clone(), stopping.clone(), frames_tx.clone());
            let settings = settings.clone();
            std::thread::Builder::new()
                .name("screen-watchdog".into())
                .spawn(move || {
                    this.watch_capture(
                        &source, encoder, &settings, &running, &stopping, &frames_tx, &last,
                    );
                })
                .map_err(|_| RefuseReason::NoEncoder)?;
        }

        // The sharing notification and its Stop action, and the portal
        // session's end, which is the platform withdrawing consent.
        let notification = Arc::new(Mutex::new(None::<u32>));
        {
            let frames_tx = frames_tx.clone();
            let notification = notification.clone();
            let summary = format!("Sharing {} with {}", settings.display, settings.viewer);
            self.handle.spawn(async move {
                if let Err(e) = sharing_notification(&summary, notification, frames_tx).await {
                    tracing::warn!(%e, "cannot show the sharing notification");
                }
            });
        }
        let portal_session = lock(&running).portal.clone();
        if let Some(p) = portal_session {
            let session_closed = {
                let frames_tx = frames_tx.clone();
                let stopping = stopping.clone();
                async move {
                    use futures_util::StreamExt as _;
                    if let Ok(mut closed) = p.session.receive_closed().await
                        && closed.next().await.is_some()
                        && !stopping.load(Ordering::SeqCst)
                    {
                        let _ = frames_tx.send(Captured::Ended(StopReason::Consent));
                    }
                }
            };
            self.handle.spawn(session_closed);
        }

        // Control: keyframes come every second on their own; a stop ends
        // the processes, the session and the notification.
        {
            let running = running.clone();
            let handle = self.handle.clone();
            self.handle.spawn(async move {
                let mut warned = false;
                while let Some(c) = control_rx.recv().await {
                    match c {
                        ShareControl::Keyframe => {
                            if !warned {
                                tracing::debug!(
                                    "keyframe asked for; the encoder sends one every second"
                                );
                                warned = true;
                            }
                        }
                        ShareControl::Stop => break,
                    }
                }
                stopping.store(true, Ordering::SeqCst);
                let portal = {
                    let mut r = lock(&running);
                    r.generation.store(true, Ordering::SeqCst);
                    for child in &mut r.children {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    r.portal.take()
                };
                if let Some(p) = portal {
                    let _ = p.session.close().await;
                }
                let id = lock(&notification).take();
                if let Some(id) = id {
                    handle.spawn(close_notification(id));
                }
                tracing::info!("screen capture stopped");
            });
        }

        Ok(Share {
            frames,
            control,
            region,
        })
    }
}

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait SharingNotifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, zbus::Variant<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;
}

/// The action key of the notification's Stop button.
const STOP_ACTION: &str = "stop";

async fn sharing_notification(
    summary: &str,
    slot: Arc<Mutex<Option<u32>>>,
    frames: mpsc::UnboundedSender<Captured>,
) -> zbus::Result<()> {
    use futures_util::StreamExt as _;
    let conn = zbus::Connection::session().await?;
    let proxy = SharingNotificationsProxy::new(&conn).await?;
    let mut actions = proxy.receive_action_invoked().await?;
    let mut hints: HashMap<&str, zbus::Variant<'_>> = HashMap::new();
    // Marked as the daemon's own, so the notifications monitor never sends
    // it to the person's other devices.
    hints.insert("x-ligature-origin", zbus::Variant::from("local"));
    hints.insert("resident", zbus::Variant::from(true));
    let id = proxy
        .notify(
            "Ligature",
            0,
            "video-display",
            summary,
            "Your screen is being shown on another of your devices.",
            &[STOP_ACTION, "Stop sharing"],
            hints,
            0,
        )
        .await?;
    *lock(&slot) = Some(id);
    while let Some(signal) = actions.next().await {
        if let Ok(args) = signal.args()
            && args.id == id
            && args.action_key == STOP_ACTION
        {
            tracing::info!("the user stopped the share from its notification");
            let _ = frames.send(Captured::Ended(StopReason::Sharer));
            break;
        }
        if frames.is_closed() {
            break;
        }
    }
    Ok(())
}

async fn close_notification(id: u32) {
    let result = async {
        let conn = zbus::Connection::session().await?;
        SharingNotificationsProxy::new(&conn)
            .await?
            .close_notification(id)
            .await
    }
    .await;
    if let Err(e) = result {
        tracing::debug!(%e, "cannot close the sharing notification");
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// The helper process: `ligatured screen-helper list|capture OUTPUT RATE`.

/// Runs the screen helper with its arguments.
///
/// # Errors
///
/// Returns [`Error::Io`] when the compositor lacks what the command needs.
pub fn run_helper(args: &[String]) -> Result<(), Error> {
    match args {
        [cmd] if cmd == "list" => {
            let list = helper::list()?;
            let bytes = frame::encode(&list, HELPER_MAX_FRAME)?;
            std::io::stdout()
                .write_all(&bytes)
                .map_err(|e| Error::Io(e.to_string()))
        }
        [cmd, output, rate] if cmd == "capture" => {
            let rate: u32 = rate
                .parse()
                .map_err(|_| Error::Decode(format!("rate {rate}")))?;
            helper::capture(output, rate.clamp(1, 60))
        }
        _ => Err(Error::Decode(
            "screen-helper list | capture OUTPUT RATE".to_owned(),
        )),
    }
}

mod helper {
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Write as _;
    use std::os::fd::AsFd as _;
    use std::os::unix::fs::FileExt as _;
    use std::time::{Duration, Instant};

    use lp_core::Error;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_buffer::WlBuffer;
    use wayland_client::protocol::wl_output::{self, WlOutput};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::protocol::wl_shm::{self, WlShm};
    use wayland_client::protocol::wl_shm_pool::WlShmPool;
    use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle, WEnum};
    use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
    use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};
    use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
        self, ZwlrScreencopyFrameV1,
    };
    use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;

    use super::{HelperList, OutputInfo};

    #[derive(Default)]
    struct Out {
        name: Option<String>,
        description: Option<String>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    }

    #[derive(Default)]
    struct FrameState {
        info: Option<(WEnum<wl_shm::Format>, u32, u32, u32)>,
        buffer_done: bool,
        ready: bool,
        failed: bool,
    }

    #[derive(Default)]
    struct State {
        outputs: HashMap<u32, Out>,
        frame: FrameState,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: wayland_client::protocol::wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<WlOutput, u32> for State {
        fn event(
            state: &mut Self,
            _: &WlOutput,
            event: wl_output::Event,
            name: &u32,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            let o = state.outputs.entry(*name).or_default();
            match event {
                wl_output::Event::Name { name } => o.name = Some(name),
                wl_output::Event::Description { description } => {
                    o.description = Some(description);
                }
                _ => {}
            }
        }
    }

    impl Dispatch<ZxdgOutputV1, u32> for State {
        fn event(
            state: &mut Self,
            _: &ZxdgOutputV1,
            event: zxdg_output_v1::Event,
            name: &u32,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            let o = state.outputs.entry(*name).or_default();
            match event {
                zxdg_output_v1::Event::LogicalPosition { x, y } => {
                    o.x = x;
                    o.y = y;
                }
                zxdg_output_v1::Event::LogicalSize { width, height } => {
                    o.width = width;
                    o.height = height;
                }
                zxdg_output_v1::Event::Name { name } if o.name.is_none() => o.name = Some(name),
                _ => {}
            }
        }
    }

    impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
        fn event(
            state: &mut Self,
            _: &ZwlrScreencopyFrameV1,
            event: zwlr_screencopy_frame_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                zwlr_screencopy_frame_v1::Event::Buffer {
                    format,
                    width,
                    height,
                    stride,
                } => state.frame.info = Some((format, width, height, stride)),
                zwlr_screencopy_frame_v1::Event::BufferDone => state.frame.buffer_done = true,
                zwlr_screencopy_frame_v1::Event::Ready { .. } => state.frame.ready = true,
                zwlr_screencopy_frame_v1::Event::Failed => state.frame.failed = true,
                _ => {}
            }
        }
    }

    macro_rules! silent {
        ($($iface:ty),* $(,)?) => {
            $(
                impl Dispatch<$iface, ()> for State {
                    fn event(
                        _: &mut Self,
                        _: &$iface,
                        _: <$iface as wayland_client::Proxy>::Event,
                        _: &(),
                        _: &Connection,
                        _: &QueueHandle<Self>,
                    ) {
                    }
                }
            )*
        };
    }
    silent!(
        ZxdgOutputManagerV1,
        ZwlrScreencopyManagerV1,
        WlShm,
        WlShmPool,
        WlBuffer
    );

    struct Session {
        conn: Connection,
        queue: wayland_client::EventQueue<State>,
        state: State,
        outputs: Vec<(u32, WlOutput)>,
        screencopy: Option<ZwlrScreencopyManagerV1>,
        shm: Option<WlShm>,
    }

    fn connect() -> Result<Session, Error> {
        let conn = Connection::connect_to_env().map_err(|e| Error::Io(format!("wayland: {e}")))?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)
            .map_err(|e| Error::Io(format!("wayland registry: {e}")))?;
        let qh = queue.handle();
        let registry = globals.registry();
        let outputs: Vec<(u32, WlOutput)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == WlOutput::interface().name)
                .map(|g| (g.name, registry.bind(g.name, g.version.min(4), &qh, g.name)))
                .collect()
        });
        let xdg: Option<ZxdgOutputManagerV1> = globals.bind(&qh, 1..=3, ()).ok();
        let screencopy: Option<ZwlrScreencopyManagerV1> = globals.bind(&qh, 1..=3, ()).ok();
        let shm: Option<WlShm> = globals.bind(&qh, 1..=1, ()).ok();
        let mut state = State::default();
        if let Some(xdg) = &xdg {
            for (name, output) in &outputs {
                xdg.get_xdg_output(output, &qh, *name);
            }
        }
        for _ in 0..2 {
            queue
                .roundtrip(&mut state)
                .map_err(|e| Error::Io(format!("wayland roundtrip: {e}")))?;
        }
        Ok(Session {
            conn,
            queue,
            state,
            outputs,
            screencopy,
            shm,
        })
    }

    pub fn list() -> Result<HelperList, Error> {
        let session = connect()?;
        let mut outputs: Vec<OutputInfo> = session
            .outputs
            .iter()
            .filter_map(|(name, _)| {
                let o = session.state.outputs.get(name)?;
                Some(OutputInfo {
                    id: o.name.clone().unwrap_or_else(|| format!("output-{name}")),
                    description: o.description.clone(),
                    x: o.x,
                    y: o.y,
                    width: u32::try_from(o.width).ok()?,
                    height: u32::try_from(o.height).ok()?,
                })
            })
            .filter(|o| o.width > 0 && o.height > 0)
            .collect();
        outputs.sort_by_key(|o| (o.y, o.x));
        drop(session.conn);
        Ok(HelperList {
            outputs,
            screencopy: session.screencopy.is_some(),
        })
    }

    /// ffmpeg's name for a `wl_shm` format's bytes in memory.
    fn pix_fmt(format: WEnum<wl_shm::Format>) -> Option<&'static str> {
        match format {
            WEnum::Value(wl_shm::Format::Xrgb8888) => Some("bgr0"),
            WEnum::Value(wl_shm::Format::Argb8888) => Some("bgra"),
            WEnum::Value(wl_shm::Format::Xbgr8888) => Some("rgb0"),
            WEnum::Value(wl_shm::Format::Abgr8888) => Some("rgba"),
            _ => None,
        }
    }

    /// Nearest-neighbour scaling of packed 32-bit pixels.
    pub fn resample(src: &[u8], from: (u32, u32), to: (u32, u32)) -> Vec<u8> {
        let (fw, fh) = (from.0 as usize, from.1 as usize);
        let (tw, th) = (to.0 as usize, to.1 as usize);
        let mut out = vec![0u8; tw * th * 4];
        if fw == 0 || fh == 0 {
            return out;
        }
        for y in 0..th {
            let sy = y * fh / th;
            for x in 0..tw {
                let sx = x * fw / tw;
                let s = (sy * fw + sx) * 4;
                let d = (y * tw + x) * 4;
                out[d..d + 4].copy_from_slice(&src[s..s + 4]);
            }
        }
        out
    }

    pub fn capture(output_name: &str, rate: u32) -> Result<(), Error> {
        let mut s = connect()?;
        let qh = s.queue.handle();
        let manager = s
            .screencopy
            .clone()
            .ok_or_else(|| Error::Io("no wlr-screencopy".to_owned()))?;
        let shm = s
            .shm
            .clone()
            .ok_or_else(|| Error::Io("no wl_shm".to_owned()))?;
        let output = s
            .outputs
            .iter()
            .find(|(name, _)| {
                s.state
                    .outputs
                    .get(name)
                    .and_then(|o| o.name.as_deref())
                    .is_some_and(|n| n == output_name)
            })
            .map(|(_, o)| o.clone())
            .ok_or_else(|| Error::Io(format!("no output {output_name}")))?;
        let version = manager.version();
        let period = Duration::from_micros(1_000_000 / u64::from(rate));
        let mut stdout = std::io::stdout().lock();
        let mut buffer: Option<(File, WlBuffer, (u32, u32, u32))> = None;
        let mut header_sent: Option<(u32, u32)> = None;
        let mut next = Instant::now();
        loop {
            s.state.frame = super::helper::FrameState::default();
            let frame = manager.capture_output(1, &output, &qh, ());
            // The buffer description, and with version 3 its end.
            while s.state.frame.info.is_none() || (version >= 3 && !s.state.frame.buffer_done) {
                if s.state.frame.failed {
                    return Err(Error::Io("screencopy failed".to_owned()));
                }
                s.queue
                    .blocking_dispatch(&mut s.state)
                    .map_err(|e| Error::Io(format!("wayland: {e}")))?;
            }
            let (format, width, height, stride) = s
                .state
                .frame
                .info
                .ok_or_else(|| Error::Io("no buffer".to_owned()))?;
            let fmt = pix_fmt(format)
                .ok_or_else(|| Error::Io(format!("unsupported screencopy format {format:?}")))?;
            // The first frame's size is the stream's; an output that changes
            // size later (a mode change, a nested compositor's window
            // resized) is scaled back to it, so the encoder never stops.
            let (out_w, out_h) = match header_sent {
                Some(size) => size,
                None => {
                    writeln!(stdout, "LPRAW {width} {height} {fmt}")
                        .and_then(|()| stdout.flush())
                        .map_err(|e| Error::Io(e.to_string()))?;
                    header_sent = Some((width, height));
                    (width, height)
                }
            };
            if buffer
                .as_ref()
                .is_none_or(|(_, _, dims)| *dims != (width, height, stride))
            {
                let size = stride as usize * height as usize;
                let fd = rustix::fs::memfd_create(
                    "ligature-screencopy",
                    rustix::fs::MemfdFlags::CLOEXEC,
                )
                .map_err(|e| Error::Io(format!("memfd: {e}")))?;
                let file = File::from(fd);
                file.set_len(size as u64)
                    .map_err(|e| Error::Io(e.to_string()))?;
                let pool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
                let format = match format {
                    WEnum::Value(f) => f,
                    WEnum::Unknown(_) => return Err(Error::Io("unknown format".to_owned())),
                };
                let wl_buffer = pool.create_buffer(
                    0,
                    width as i32,
                    height as i32,
                    stride as i32,
                    format,
                    &qh,
                    (),
                );
                pool.destroy();
                buffer = Some((file, wl_buffer, (width, height, stride)));
            }
            let (file, wl_buffer, _) = buffer
                .as_ref()
                .ok_or_else(|| Error::Io("no buffer".to_owned()))?;
            frame.copy(wl_buffer);
            while !s.state.frame.ready && !s.state.frame.failed {
                s.queue
                    .blocking_dispatch(&mut s.state)
                    .map_err(|e| Error::Io(format!("wayland: {e}")))?;
            }
            frame.destroy();
            if s.state.frame.failed {
                return Err(Error::Io("screencopy failed".to_owned()));
            }
            // Packed rows, as ffmpeg's raw input wants.
            let row = width as usize * 4;
            let mut packed = vec![0u8; row * height as usize];
            let mut line = vec![0u8; stride as usize];
            for y in 0..height as usize {
                file.read_exact_at(&mut line, (y * stride as usize) as u64)
                    .map_err(|e| Error::Io(e.to_string()))?;
                packed[y * row..(y + 1) * row].copy_from_slice(&line[..row]);
            }
            let packed = if (width, height) == (out_w, out_h) {
                packed
            } else {
                resample(&packed, (width, height), (out_w, out_h))
            };
            if stdout
                .write_all(&packed)
                .and_then(|()| stdout.flush())
                .is_err()
            {
                // The encoder went away: the share is over.
                return Ok(());
            }
            next += period;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                next = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(w: u32, h: u32, rate: u32) -> Settings {
        Settings {
            display: "DP-1".to_owned(),
            codec: Codec::H264,
            width: w,
            height: h,
            frame_rate: rate,
            viewer: "mac".to_owned(),
        }
    }

    #[test]
    fn a_quiet_capture_is_restarted_three_times_then_the_share_ends() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut w = StallWatch::default();
        assert_eq!(w.check(t0, s(1)), Stall::Fine);
        assert_eq!(w.check(t0 + s(4), s(4)), Stall::Restart);
        assert_eq!(w.check(t0 + s(9), s(4)), Stall::Restart);
        assert_eq!(w.check(t0 + s(14), s(5)), Stall::Restart);
        assert_eq!(w.check(t0 + s(19), s(4)), Stall::GiveUp);
        // A capture that ran a minute after its restart has its count reset.
        let mut w = StallWatch::default();
        for i in 0..3 {
            assert_eq!(w.check(t0 + s(5 * i), s(4)), Stall::Restart);
        }
        assert_eq!(w.check(t0 + s(10 + 59), s(0)), Stall::Fine);
        assert_eq!(w.restarts, 3, "not yet a minute");
        assert_eq!(w.check(t0 + s(10 + 60), s(0)), Stall::Fine);
        assert_eq!(w.check(t0 + s(80), s(4)), Stall::Restart);
        assert_eq!(w.restarts, 1);
    }

    #[test]
    fn access_units_split_at_the_delimiters_and_keyframes_are_found() {
        let aud = [0u8, 0, 0, 1, 0x09, 0xf0];
        let sps = [0u8, 0, 0, 1, 0x67, 1, 2];
        let idr = [0u8, 0, 1, 0x65, 9, 9, 9];
        let p = [0u8, 0, 0, 1, 0x41, 7, 7];
        let mut stream = Vec::new();
        stream.extend_from_slice(&aud);
        stream.extend_from_slice(&sps);
        stream.extend_from_slice(&idr);
        stream.extend_from_slice(&aud);
        stream.extend_from_slice(&p);
        stream.extend_from_slice(&aud);
        let mut units = AccessUnits::default();
        // Fed a byte at a time, the units come out whole and in order.
        let mut out = Vec::new();
        for b in &stream {
            out.extend(units.push(&[*b]));
        }
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], [aud.as_slice(), &sps, &idr].concat());
        assert!(is_keyframe(&out[0]));
        assert_eq!(out[1], [aud.as_slice(), &p].concat());
        assert!(!is_keyframe(&out[1]));
        assert_eq!(units.finish(), Some(aud.to_vec()));
        assert_eq!(units.finish(), None);
    }

    #[test]
    fn the_encoder_is_told_the_input_the_size_the_rate_and_the_keyframe_interval() {
        let input = RawInput {
            width: 2560,
            height: 1440,
            pix_fmt: "bgr0".to_owned(),
        };
        let args = encoder_args(Encoder::X264, &input, &settings(1280, 720, 30));
        let joined = args.join(" ");
        assert!(joined.contains(
            "-pix_fmt bgr0 -video_size 2560x1440 -framerate 30 \
             -use_wallclock_as_timestamps 1 -i pipe:0"
        ));
        assert!(joined.contains("-vf scale=1280:720 -c:v libx264"));
        assert!(joined.contains("aud=1:repeat-headers=1"));
        assert!(joined.contains(&format!("-force_key_frames {KEYFRAME_EVERY_SECOND}")));
        assert!(joined.contains("-bf 0"));
        assert!(joined.contains("-fps_mode passthrough"));
        // x264 holds a quality, not a rate, under a cap of four times the
        // target with a buffer of a second, so a keyframe of text is sharp.
        assert!(joined.contains("-preset superfast -tune zerolatency"));
        assert!(joined.contains(&format!("-crf {X264_CRF}")));
        assert!(!joined.contains("-b:v"));
        let cap = 4 * bit_rate(1280, 720, 30);
        assert!(joined.contains(&format!("-maxrate {cap} -bufsize {cap}")));
        assert!(joined.ends_with("-f h264 pipe:1"));
        let vaapi = encoder_args(Encoder::Vaapi, &input, &settings(1280, 720, 30)).join(" ");
        assert!(vaapi.contains("-vaapi_device /dev/dri/renderD128"));
        assert!(vaapi.contains("scale=1280:720,format=nv12,hwupload -c:v h264_vaapi -aud 1"));
        assert!(vaapi.contains(&format!("-b:v {}", bit_rate(1280, 720, 30))));
        assert!(vaapi.contains(&format!("-maxrate {cap} -bufsize {cap}")));
        assert_eq!(bit_rate(1280, 720, 30), 2_211_840);
        assert_eq!(bit_rate(64, 64, 1), 1_000_000);
        assert_eq!(bit_rate(7680, 4320, 60), 20_000_000);
        let gst = pipewire_args(7, 42, &settings(1280, 720, 25)).join(" ");
        assert!(gst.contains("pipewiresrc fd=7 path=42"));
        // A still screen is sent again once a second, a changing one at most
        // at the agreed rate, never padded to it.
        assert!(gst.contains("keepalive-time=1000"));
        assert!(gst.contains("videorate drop-only=true max-rate=25"));
        assert!(gst.contains("format=BGRx,width=1280,height=720 !"));
        assert!(!gst.contains("framerate"));
    }

    #[test]
    fn displays_and_regions_come_from_the_layout() {
        let list = HelperList {
            outputs: vec![
                OutputInfo {
                    id: "HDMI-A-1".into(),
                    description: None,
                    x: 2560,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                OutputInfo {
                    id: "DP-1".into(),
                    description: Some("Dell".into()),
                    x: 0,
                    y: 0,
                    width: 2560,
                    height: 1440,
                },
            ],
            screencopy: true,
        };
        let displays = list.displays();
        assert_eq!(displays.len(), 2);
        assert!(displays.iter().find(|d| d.id == "DP-1").unwrap().primary);
        assert!(
            !displays
                .iter()
                .find(|d| d.id == "HDMI-A-1")
                .unwrap()
                .primary
        );
        let r = list.region("HDMI-A-1").unwrap();
        assert!((r.x - 2560.0 / 4480.0).abs() < 1e-9);
        assert_eq!(r.y, 0.0);
        assert!((r.width - 1920.0 / 4480.0).abs() < 1e-9);
        assert!((r.height - 1080.0 / 1440.0).abs() < 1e-9);
        assert_eq!(list.at(2560, 0).unwrap().id, "HDMI-A-1");
        assert!(list.region("nope").is_none());
    }

    #[test]
    fn a_resized_output_is_scaled_back_to_the_stream_s_size() {
        // A 2x1 picture, red then blue, scaled to 4x2.
        let src = [0, 0, 255, 0, 255, 0, 0, 0];
        let out = helper::resample(&src, (2, 1), (4, 2));
        assert_eq!(out.len(), 4 * 2 * 4);
        assert_eq!(&out[0..4], &[0, 0, 255, 0]);
        assert_eq!(&out[4..8], &[0, 0, 255, 0]);
        assert_eq!(&out[8..12], &[255, 0, 0, 0]);
        assert_eq!(
            &out[16..20],
            &[0, 0, 255, 0],
            "the second row repeats the first"
        );
        assert_eq!(helper::resample(&src, (2, 1), (1, 1)), vec![0, 0, 255, 0]);
    }

    #[test]
    fn the_helper_header_and_listing_parse() {
        let mut header: &[u8] = b"LPRAW 1280 720 bgr0\nrest";
        assert_eq!(
            read_raw_header(&mut header).unwrap(),
            RawInput {
                width: 1280,
                height: 720,
                pix_fmt: "bgr0".to_owned()
            }
        );
        assert_eq!(header, b"rest");
        assert!(read_raw_header(&mut &b"NOPE\n"[..]).is_err());
        let list = HelperList {
            outputs: vec![],
            screencopy: true,
        };
        let bytes = frame::encode(&list, HELPER_MAX_FRAME).unwrap();
        assert_eq!(read_list(&bytes).unwrap(), list);
    }
}
