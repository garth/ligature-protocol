//! The camera capability on Linux (milestone 10, LP-207): the cameras
//! PipeWire lists, shared with the default microphone, and a camera received
//! from another device presented to this machine's programs as a camera and
//! a microphone.
//!
//! Nothing here links a media library, as nothing in the screen's capture
//! does. A share is GStreamer's `pipewiresrc` reading the camera's node into
//! raw pictures of the agreed size and rate, which the daemon copies into the
//! live encoder of milestone 8; a machine whose cameras PipeWire does not
//! list falls back to plain V4L2 devices read by ffmpeg. The microphone is
//! the default source, encoded as the screen's sound is
//! ([`crate::screen::sound`]).
//!
//! **A share reads the camera it names and no other.** PipeWire's session
//! manager links a stream whose target is gone to the default camera
//! instead, which would show a different camera than the one asked for, so
//! every reader sets `node.dont-fallback`: a camera that has gone ends the
//! share.
//!
//! Presented, a received camera is decoded by ffmpeg and offered by
//! GStreamer's `pipewiresink` as a PipeWire node of the class `Video/Source`
//! with the role `Camera`, which is what the Camera portal and the programs
//! that read cameras through PipeWire list, and through a `v4l2loopback`
//! device where the kernel module is loaded, for the programs that read
//! V4L2 alone. Its sound is played by ffmpeg into one end of a `pw-loopback`
//! pair whose other end is a node of the class `Audio/Source`, which every
//! program lists among the microphones, through PulseAudio's interface too
//! (a source GStreamer offers is not read cleanly through it). Neither is
//! given a priority, so neither becomes the default over the machine's own.
//!
//! Since the compositor shows no indicator, a share posts the notification
//! a screen's does, "Sharing `camera` with `device`", with its Stop action,
//! and does not start when it cannot be shown.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_core::Error;
use lp_core::camera::{Camera, Facing};
use lp_core::screen::{
    AudioFormat, Display, Format, RefuseReason, ScreenParams, SourceKind, Started, StopReason,
};
use tokio::sync::mpsc;

use crate::screen::encode::{Encoder, LiveEncoder, RawInput, probe_encoder};
use crate::screen::sound::{self, PRESENTED_MICROPHONE};
use crate::screen::{
    Capture, Captured, Presented, Presenter, Settings, Share, ShareControl, ogg, probe_decoder,
};
use crate::screen_linux::{
    Notice, NotificationGuard, close_notification, relay, sharing_notification,
};

/// The start of the node name of a camera this daemon presents, followed by
/// the sending device's name: such a node is never listed as a camera to
/// share, or one device's camera would come back to it as another's.
pub const PRESENTED_CAMERA: &str = "ligature_camera.";

/// The largest picture a camera is captured at. A camera that offers more is
/// read at the largest size within it.
const MAX_SIZE: (u32, u32) = (1920, 1080);
/// The size assumed for a camera that does not say its sizes.
const UNSTATED_SIZE: (u32, u32) = (1280, 720);
/// The frame rate assumed for a camera that does not say its rates.
const UNSTATED_RATE: u32 = 30;
/// The highest frame rate a camera is listed with.
const MAX_RATE: u32 = 60;
/// How often the cameras are listed again, so one plugged in is found.
const LIST_EVERY: Duration = Duration::from_secs(10);
/// A camera sends a picture at its rate always, so one that has sent
/// nothing for this long has gone, or never started: the share ends.
const STALL: Duration = Duration::from_secs(6);
/// How many V4L2 devices are asked for their formats when PipeWire lists no
/// camera.
const V4L2_PROBED: usize = 8;
/// How many frames or packets wait for a presenting sink before it is taken
/// for stuck and dropped.
const SINK_QUEUE: usize = 120;
/// How long a presented microphone's output has to appear.
const FEED_WAIT: Duration = Duration::from_secs(3);

/// Where a camera's pictures come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A PipeWire node, by its name.
    PipeWire {
        /// The node's `node.name`.
        target: String,
    },
    /// A V4L2 device read by ffmpeg, where PipeWire lists no camera.
    V4l2 {
        /// The device file.
        device: PathBuf,
    },
}

/// A camera found on this machine and how it is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// What the peers are told.
    pub camera: Camera,
    /// Where its pictures come from.
    pub origin: Origin,
}

/// The largest of `sizes` within [`MAX_SIZE`], or the smallest when none
/// fits.
fn best_size(sizes: impl Iterator<Item = (u32, u32)>) -> Option<(u32, u32)> {
    let area = |(w, h): &(u32, u32)| u64::from(*w) * u64::from(*h);
    let sizes: Vec<(u32, u32)> = sizes.filter(|(w, h)| *w > 0 && *h > 0).collect();
    sizes
        .iter()
        .filter(|(w, h)| *w <= MAX_SIZE.0 && *h <= MAX_SIZE.1)
        .max_by_key(|s| area(s))
        .or_else(|| sizes.iter().min_by_key(|s| area(s)))
        .copied()
}

/// A size in `pw-dump`'s JSON: the value itself, or a choice's default.
fn size_of(value: &serde_json::Value) -> Option<(u32, u32)> {
    let value = value.get("default").unwrap_or(value);
    Some((
        u32::try_from(value.get("width")?.as_u64()?).ok()?,
        u32::try_from(value.get("height")?.as_u64()?).ok()?,
    ))
}

/// A frame rate in `pw-dump`'s JSON, to the nearest whole frame.
fn rate_of(value: &serde_json::Value) -> Option<u32> {
    let value = value.get("default").unwrap_or(value);
    let (num, denom) = (value.get("num")?.as_u64()?, value.get("denom")?.as_u64()?);
    let rate = u32::try_from((num + denom / 2).checked_div(denom)?).ok()?;
    (rate > 0).then_some(rate.min(MAX_RATE))
}

/// The cameras in `pw-dump`'s output: every node of the class `Video/Source`
/// that offers raw pictures and is not one this daemon presents, the
/// session's preferred one first.
#[must_use]
pub fn pipewire_cameras(dump: &str) -> Vec<Found> {
    let Ok(serde_json::Value::Array(objects)) = serde_json::from_str(dump) else {
        return Vec::new();
    };
    let mut found: Vec<(i64, Found)> = objects
        .iter()
        .filter_map(|object| {
            let info = object.get("info")?;
            let props = info.get("props")?;
            let text = |key: &str| props.get(key).and_then(serde_json::Value::as_str);
            if text("media.class")? != "Video/Source" {
                return None;
            }
            let name = text("node.name")?;
            if name.starts_with(PRESENTED_CAMERA) {
                return None;
            }
            let formats: Vec<&serde_json::Value> = info
                .get("params")
                .and_then(|p| p.get("EnumFormat"))
                .and_then(serde_json::Value::as_array)
                .map(|formats| {
                    formats
                        .iter()
                        .filter(|f| {
                            f.get("mediaSubtype").and_then(serde_json::Value::as_str) == Some("raw")
                        })
                        .collect()
                })
                .unwrap_or_default();
            // A node that says its formats and has no raw one cannot be read
            // by the pipeline here; one that says none is taken on trust.
            let stated = info
                .get("params")
                .and_then(|p| p.get("EnumFormat"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|formats| !formats.is_empty());
            if stated && formats.is_empty() {
                return None;
            }
            let (width, height) = best_size(formats.iter().filter_map(|f| size_of(f.get("size")?)))
                .unwrap_or(UNSTATED_SIZE);
            let rates: BTreeSet<u32> = formats
                .iter()
                .filter(|f| {
                    f.get("size")
                        .and_then(size_of)
                        .is_none_or(|size| size == (width, height))
                })
                .filter_map(|f| rate_of(f.get("framerate")?))
                .collect();
            let frame_rates = if rates.is_empty() {
                vec![UNSTATED_RATE]
            } else {
                rates.into_iter().rev().collect()
            };
            let facing = match text("api.libcamera.location") {
                Some("front") => Facing::Front,
                Some("back") => Facing::Back,
                _ => Facing::External,
            };
            let priority = props
                .get("priority.session")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            Some((
                priority,
                Found {
                    camera: Camera {
                        id: name.to_owned(),
                        name: text("node.description")
                            .or_else(|| text("node.nick"))
                            .map(str::to_owned),
                        facing,
                        width,
                        height,
                        frame_rates,
                    },
                    origin: Origin::PipeWire {
                        target: name.to_owned(),
                    },
                },
            ))
        })
        .collect();
    found.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.camera.id.cmp(&b.1.camera.id))
    });
    found.into_iter().map(|(_, found)| found).collect()
}

/// The best size among those `ffmpeg -f v4l2 -list_formats all` prints for a
/// device, or `None` for one that captures nothing ffmpeg reads (a sensor's
/// raw Bayer node, a metadata node).
#[must_use]
pub fn v4l2_size(listing: &str) -> Option<(u32, u32)> {
    let sizes = listing
        .lines()
        .filter_map(|line| {
            // "… Raw       :     yuyv422 :           YUYV 4:2:2 : 640x480 1280x720"
            // "… Compressed:       mjpeg :          Motion-JPEG : 640x480 1920x1080"
            let kind = line.find("Raw").or_else(|| line.find("Compressed"))?;
            let (_, rest) = line[kind..].split_once(':')?;
            let format = rest.split(" : ").next()?.trim();
            let sizes = rest.rsplit(" : ").next()?;
            (format != "Unsupported" && rest.matches(" : ").count() >= 2).then_some(sizes)
        })
        .flat_map(str::split_whitespace)
        .filter_map(|size| {
            let (w, h) = size.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        });
    best_size(sizes)
}

/// A value quoted for `gst-launch-1.0`'s pipeline parser.
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The GStreamer pipeline that reads the camera node `target`, and only it,
/// into raw BGRx pictures of the share's size and rate on stdout.
/// `videorate` gives the agreed rate whatever the camera's own, and
/// `videoscale` keeps the camera's shape within the agreed picture.
///
/// The queue before the pipe is leaky: when the daemon stops reading for a
/// moment, as it does while an encoder is started again for a keyframe, the
/// newest pictures replace the waiting ones and the camera is never held
/// up. Held up, the reader keeps the camera's buffers, the camera runs out
/// of them and its pictures stop for good.
#[must_use]
pub fn pipewire_args(target: &str, settings: &Settings) -> Vec<String> {
    let mut args = vec![
        "-q".to_owned(),
        "pipewiresrc".to_owned(),
        format!("target-object={}", quoted(target)),
        // Never another camera in this one's place, at the start or when
        // this one goes away.
        "stream-properties=\"props,node.dont-fallback=true,node.dont-reconnect=true\"".to_owned(),
        "do-timestamp=true".to_owned(),
    ];
    args.extend(
        format!(
            "! videorate ! videoconvert ! videoscale add-borders=true \
             ! video/x-raw,format=BGRx,width={},height={},framerate={}/1,pixel-aspect-ratio=1/1 \
             ! queue leaky=downstream max-size-buffers=2 max-size-bytes=0 max-size-time=0 \
             ! fdsink fd=1 sync=false",
            settings.width,
            settings.height,
            settings.frame_rate.max(1)
        )
        .split_whitespace()
        .map(str::to_owned),
    );
    args
}

/// The ffmpeg that reads the V4L2 `device` at its `native` size into raw
/// BGRx pictures of the share's size and rate on stdout, the camera's shape
/// kept within the agreed picture.
#[must_use]
pub fn v4l2_args(device: &Path, native: (u32, u32), settings: &Settings) -> Vec<String> {
    let (w, h, rate) = (settings.width, settings.height, settings.frame_rate.max(1));
    vec![
        "-hide_banner".to_owned(),
        "-loglevel".to_owned(),
        "error".to_owned(),
        "-nostdin".to_owned(),
        "-f".to_owned(),
        "v4l2".to_owned(),
        "-framerate".to_owned(),
        rate.to_string(),
        "-video_size".to_owned(),
        format!("{}x{}", native.0, native.1),
        "-i".to_owned(),
        device.display().to_string(),
        "-vf".to_owned(),
        format!(
            "scale={w}:{h}:force_original_aspect_ratio=decrease,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={rate}"
        ),
        "-pix_fmt".to_owned(),
        "bgr0".to_owned(),
        "-f".to_owned(),
        "rawvideo".to_owned(),
        "pipe:1".to_owned(),
    ]
}

/// The plain V4L2 capture devices, asked through ffmpeg: the first node of
/// each device that is not a loopback, up to [`V4L2_PROBED`].
fn v4l2_cameras() -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") else {
        return Vec::new();
    };
    let mut nodes: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with("video"))
        .filter(|name| {
            let sys = Path::new("/sys/class/video4linux").join(name);
            let first =
                std::fs::read_to_string(sys.join("index")).is_ok_and(|index| index.trim() == "0");
            // A loopback device is where a camera is presented, not one.
            let real = std::fs::canonicalize(&sys)
                .is_ok_and(|path| !path.starts_with("/sys/devices/virtual"));
            first && real
        })
        .collect();
    nodes.sort_by_key(|name| name[5..].parse::<u32>().unwrap_or(u32::MAX));
    nodes
        .into_iter()
        .take(V4L2_PROBED)
        .filter_map(|node| {
            let device = Path::new("/dev").join(&node);
            let out = Command::new("ffmpeg")
                .args(["-hide_banner", "-f", "v4l2", "-list_formats", "all", "-i"])
                .arg(&device)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .output()
                .ok()?;
            let (width, height) = v4l2_size(&String::from_utf8_lossy(&out.stderr))?;
            let name = std::fs::read_to_string(
                Path::new("/sys/class/video4linux").join(&node).join("name"),
            )
            .ok()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
            Some(Found {
                camera: Camera {
                    id: format!("v4l2-{node}"),
                    name,
                    facing: Facing::External,
                    width,
                    height,
                    frame_rates: vec![UNSTATED_RATE],
                },
                origin: Origin::V4l2 { device },
            })
        })
        .collect()
}

/// Whether GStreamer has `element`.
fn gst_has(element: &str) -> bool {
    Command::new("gst-inspect-1.0")
        .args(["--exists", element])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[derive(Debug, Default, Clone)]
struct Probe {
    encoder: Option<Encoder>,
    pipewiresrc: bool,
    microphone: bool,
    found: Vec<Found>,
}

/// The Linux camera capture. Implements [`Capture`] for the camera family.
#[derive(Clone)]
pub struct LinuxCamera {
    handle: tokio::runtime::Handle,
    probe: Arc<Mutex<Probe>>,
    /// The one camera listed, for a test that must reach no other.
    only: Option<String>,
    /// The source captured as the microphone instead of the default one,
    /// for a test that must not open the machine's own.
    microphone: Option<String>,
}

impl std::fmt::Debug for LinuxCamera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LinuxCamera")
    }
}

impl LinuxCamera {
    /// Starts the capture: probes the tools and lists the cameras in the
    /// background. Called within the daemon's runtime.
    #[must_use]
    pub fn start() -> Arc<Self> {
        Self::start_with(None, None)
    }

    /// As [`LinuxCamera::start`], listing the camera `only` and no other
    /// when one is named, and capturing the source `microphone` instead of
    /// the default one.
    #[must_use]
    pub fn start_with(only: Option<String>, microphone: Option<String>) -> Arc<Self> {
        let this = Arc::new(Self {
            handle: tokio::runtime::Handle::current(),
            probe: Arc::new(Mutex::new(Probe::default())),
            only,
            microphone,
        });
        let prober = this.clone();
        std::thread::Builder::new()
            .name("camera-probe".into())
            .spawn(move || prober.probe_loop())
            .expect("spawn the camera probe");
        this
    }

    /// Lists the cameras now: PipeWire's, or the plain V4L2 devices `v4l2`
    /// found when it has none.
    fn list_now(&self, v4l2: &[Found]) -> Vec<Found> {
        let dump = Command::new("pw-dump")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
        let mut found = dump.as_deref().map(pipewire_cameras).unwrap_or_default();
        if found.is_empty() {
            found = v4l2.to_vec();
        }
        if let Some(only) = &self.only {
            found.retain(|f| &f.camera.id == only);
        }
        found
    }

    fn probe_loop(self: Arc<Self>) {
        let encoder = probe_encoder();
        let pipewiresrc = gst_has("pipewiresrc");
        let microphone = self.microphone.is_some() || sound::probe_microphone();
        tracing::info!(
            ?encoder,
            pipewiresrc,
            microphone,
            "camera sharing tools probed"
        );
        // The V4L2 devices are asked once for each set of them: asking is
        // slow, and they change only when one is plugged in or out.
        let mut nodes: Option<Vec<std::ffi::OsString>> = None;
        let mut v4l2: Vec<Found> = Vec::new();
        loop {
            let mut found = self.list_now(&[]);
            if found.is_empty() && self.only.is_none() {
                let now: Vec<std::ffi::OsString> = std::fs::read_dir("/sys/class/video4linux")
                    .map(|entries| {
                        let mut names: Vec<_> = entries
                            .filter_map(Result::ok)
                            .map(|e| e.file_name())
                            .collect();
                        names.sort();
                        names
                    })
                    .unwrap_or_default();
                if nodes.as_ref() != Some(&now) {
                    v4l2 = v4l2_cameras();
                    nodes = Some(now);
                }
                found.clone_from(&v4l2);
            }
            // A PipeWire camera needs the element that reads it.
            found.retain(|f| pipewiresrc || matches!(f.origin, Origin::V4l2 { .. }));
            {
                let mut probe = lock(&self.probe);
                probe.encoder = encoder;
                probe.pipewiresrc = pipewiresrc;
                probe.microphone = microphone;
                if probe.found != found {
                    tracing::info!(
                        cameras = ?found.iter().map(|f| f.camera.id.as_str()).collect::<Vec<_>>(),
                        "cameras changed"
                    );
                }
                probe.found = found;
            }
            std::thread::sleep(LIST_EVERY);
        }
    }

    /// The process that writes the camera's raw pictures to its stdout.
    fn producer(found: &Found, settings: &Settings) -> Result<Child, RefuseReason> {
        let mut command = match &found.origin {
            Origin::PipeWire { target } => {
                let mut c = Command::new("gst-launch-1.0");
                c.args(pipewire_args(target, settings));
                c
            }
            Origin::V4l2 { device } => {
                let mut c = Command::new("ffmpeg");
                c.args(v4l2_args(
                    device,
                    (found.camera.width, found.camera.height),
                    settings,
                ));
                c
            }
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                tracing::warn!(%e, "cannot start the camera's reader");
                RefuseReason::NoEncoder
            })
    }
}

impl Capture for LinuxCamera {
    fn params(&self) -> ScreenParams {
        let probe = lock(&self.probe);
        let shares = probe.encoder.is_some() && !probe.found.is_empty();
        ScreenParams {
            shares,
            views: false,
            // A camera's picture is 4:2:0 at its source: nothing to gain
            // from the 4:4:4 a screen of text is sent in.
            encodes: if probe.encoder.is_some() {
                vec![Format::H264]
            } else {
                Vec::new()
            },
            audio_encodes: if probe.microphone {
                vec![AudioFormat::OPUS_MONO]
            } else {
                Vec::new()
            },
            ..ScreenParams::default()
        }
    }

    fn displays(&self) -> Vec<Display> {
        Vec::new()
    }

    fn cameras(&self) -> Vec<Camera> {
        lock(&self.probe)
            .found
            .iter()
            .map(|f| f.camera.clone())
            .collect()
    }

    fn start(&self, settings: &Settings) -> Result<Share, RefuseReason> {
        if settings.kind != SourceKind::Camera {
            return Err(RefuseReason::NoDisplay);
        }
        let probe = lock(&self.probe).clone();
        let encoder = probe.encoder.ok_or(RefuseReason::NoEncoder)?;
        // Listed again now: the camera named must be there at this moment,
        // since its reader takes no other in its place.
        let found = self
            .list_now(&probe.found)
            .into_iter()
            .find(|f| f.camera.id == settings.display)
            .ok_or(RefuseReason::NoDisplay)?;
        let with_sound = settings.audio.is_some() && probe.microphone;

        // The sharing notification first, as for a screen: a camera never
        // runs unseen, and a share that cannot show it does not start.
        let (frames_tx, frames) = mpsc::unbounded_channel::<Captured>();
        let notification = Arc::new(Mutex::new(None::<u32>));
        let (posted_tx, posted_rx) = tokio::sync::oneshot::channel::<zbus::Result<u32>>();
        {
            let frames_tx = frames_tx.clone();
            let notification = notification.clone();
            let notice = Notice {
                summary: format!(
                    "{} {} with {}",
                    if settings.cast { "Casting" } else { "Sharing" },
                    found.camera.name.as_deref().unwrap_or(&found.camera.id),
                    settings.viewer
                ),
                body: if with_sound {
                    "Your camera and microphone are on, and shown and heard on another of your devices; this notice stays while they are."
                } else {
                    "Your camera is on and shown on another of your devices; this notice stays while it is."
                },
                icon: "camera-web",
            };
            self.handle.spawn(async move {
                sharing_notification(&notice, notification, frames_tx, posted_tx).await;
            });
        }
        if !matches!(posted_rx.blocking_recv(), Ok(Ok(_))) {
            tracing::warn!("cannot show the sharing notification; refusing the camera");
            return Err(RefuseReason::Consent);
        }
        let mut notice = NotificationGuard {
            slot: notification.clone(),
            handle: self.handle.clone(),
            armed: true,
        };

        let mut producer = Self::producer(&found, settings)?;
        let input = RawInput {
            width: settings.width,
            height: settings.height,
            pix_fmt: "bgr0".to_owned(),
        };
        let stop_producer = |producer: &mut Child| {
            let _ = producer.kill();
            let _ = producer.wait();
        };
        let last = Arc::new(Mutex::new(Instant::now()));
        let live = LiveEncoder::start(
            encoder,
            input.clone(),
            settings,
            frames_tx.clone(),
            last.clone(),
        )
        .inspect_err(|_| stop_producer(&mut producer))?;
        let generation = Arc::new(AtomicBool::new(false));
        if let Err(e) = relay(
            &mut producer,
            &input,
            live.clone(),
            frames_tx.clone(),
            generation.clone(),
        ) {
            live.stop();
            stop_producer(&mut producer);
            return Err(e);
        }
        // The microphone, when agreed: one that cannot start leaves the
        // camera silent rather than refused.
        let microphone = settings.audio.filter(|_| with_sound).and_then(|format| {
            sound::Sound::start_microphone(format, self.microphone.as_deref(), frames_tx.clone())
                .inspect_err(
                    |e| tracing::warn!(%e, "cannot capture the microphone; sharing silent"),
                )
                .ok()
        });
        tracing::info!(
            camera = %settings.display,
            origin = ?found.origin,
            ?encoder,
            microphone = microphone.is_some(),
            size = format!("{}x{}", input.width, input.height),
            rate = settings.frame_rate,
            "camera capture started"
        );

        // A camera that sends nothing has gone or never started (its reader
        // waits for ever on a node that is not there): the share ends.
        let stopping = Arc::new(AtomicBool::new(false));
        {
            let (stopping, frames_tx) = (stopping.clone(), frames_tx.clone());
            std::thread::Builder::new()
                .name("camera-watchdog".into())
                .spawn(move || {
                    while !stopping.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_secs(1));
                        if lock(&last).elapsed() > STALL && !stopping.load(Ordering::SeqCst) {
                            tracing::warn!("the camera sent nothing; ending the share");
                            let _ = frames_tx.send(Captured::Ended(StopReason::Error));
                            return;
                        }
                    }
                })
                .map_err(|_| RefuseReason::NoEncoder)?;
        }

        // Control: a keyframe from the encoder started again; a stop ends
        // the processes and the notification.
        let (control, mut control_rx) = mpsc::unbounded_channel::<ShareControl>();
        {
            let handle = self.handle.clone();
            let live = live.clone();
            let mut running = Some((producer, microphone));
            self.handle.spawn(async move {
                while let Some(c) = control_rx.recv().await {
                    match c {
                        ShareControl::Keyframe => live.keyframe(),
                        ShareControl::Stop => break,
                    }
                }
                stopping.store(true, Ordering::SeqCst);
                generation.store(true, Ordering::SeqCst);
                if let Some((mut producer, microphone)) = running.take() {
                    let _ = producer.kill();
                    let _ = producer.wait();
                    drop(microphone);
                }
                live.stop();
                let id = lock(&notification).take();
                if let Some(id) = id {
                    handle.spawn(close_notification(id));
                }
                tracing::info!("camera capture stopped");
            });
        }

        notice.armed = false;
        Ok(Share {
            frames,
            control,
            region: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Presenting a received camera.

/// A device's name made safe inside a quoted GStreamer property: letters,
/// digits, spaces and a little punctuation.
fn plain(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                ' '
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        "device".to_owned()
    } else {
        cleaned
    }
}

/// A device's name as the end of a PipeWire node name.
fn slug(name: &str) -> String {
    plain(name)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// What a presented camera is called in a program's list of cameras.
#[must_use]
pub fn camera_label(from: &str) -> String {
    format!("Ligature camera ({})", plain(from))
}

/// What a presented camera's sound is called in a list of microphones.
#[must_use]
pub fn microphone_label(from: &str) -> String {
    format!("Ligature microphone ({})", plain(from))
}

/// The form a presented PipeWire camera's pictures take, as ffmpeg names it:
/// packed 4:2:2, which every program that reads a webcam takes. Its picture
/// is one block of memory. A planar form such as I420 leaves `pipewiresink`
/// as a block for each plane, and the capture code the browsers share
/// (libwebrtc's) reads the first block alone and drops a picture that is not
/// whole in it: Firefox listed such a camera, linked to it and showed nothing.
pub const NODE_PIXELS: &str = "yuyv422";

/// The same form as GStreamer names it.
const NODE_FORMAT: &str = "yuy2";

/// The form a loopback device's pictures take: V4L2 holds a planar picture in
/// one buffer, so this is the decoder's own.
pub const LOOPBACK_PIXELS: &str = "yuv420p";

/// The ffmpeg that decodes the share's H.264 from its stdin into raw pictures
/// of the share's size in the form `pixels`, each as it comes, to `output`.
#[must_use]
pub fn decode_args(started: &Started, pixels: &str, output: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-flags",
        "low_delay",
        "-probesize",
        "32",
        "-analyzeduration",
        "0",
        "-f",
        "h264",
        "-i",
        "pipe:0",
        "-vf",
    ]
    .map(str::to_owned)
    .to_vec();
    args.push(format!("scale={}:{}", started.width, started.height));
    args.extend(["-pix_fmt", pixels, "-fps_mode", "passthrough"].map(str::to_owned));
    args.extend(output.iter().map(|s| (*s).to_owned()));
    args
}

/// The GStreamer pipeline that offers the raw pictures on its stdin, in the
/// form [`NODE_PIXELS`], as a PipeWire camera node. The queue drops a picture rather than hold the
/// decoder back when a reader is slow, and `sync=false` passes each on as it
/// comes.
#[must_use]
pub fn camera_node_args(from: &str, started: &Started) -> Vec<String> {
    let frame = u64::from(started.width) * u64::from(started.height) * 2;
    let mut args: Vec<String> = format!(
        "-q fdsrc fd=0 blocksize={frame} \
         ! rawvideoparse format={NODE_FORMAT} width={} height={} framerate={}/1 \
         ! queue leaky=downstream max-size-buffers=2 max-size-bytes=0 max-size-time=0 \
         ! pipewiresink mode=provide sync=false",
        started.width,
        started.height,
        started.frame_rate.max(1)
    )
    .split_whitespace()
    .map(str::to_owned)
    .collect();
    args.push(format!(
        "stream-properties=\"props,media.class=Video/Source,media.role=Camera,\
         node.name={PRESENTED_CAMERA}{},node.description=\\\"{}\\\"\"",
        slug(from),
        camera_label(from)
    ));
    args
}

/// The name of the output a presented camera's sound is played into, whose
/// other end is its microphone.
fn feed_name(from: &str) -> String {
    format!("ligature_microphone_feed.{}", slug(from))
}

/// The `pw-loopback` that makes a presented camera's microphone: an output
/// the sound is played into and a source that programs list among the
/// microphones, the one passed straight to the other. The pair takes the
/// sound in real time whether or not a program listens, so a program that
/// starts listening hears now, not what queued while nobody did.
#[must_use]
pub fn loopback_args(from: &str, channels: u8) -> Vec<String> {
    let position = if channels == 1 {
        "[ MONO ]"
    } else {
        "[ FL FR ]"
    };
    vec![
        format!(
            "--capture-props={{ media.class = Audio/Sink node.name = {} \
             node.description = \"Ligature microphone feed ({})\" audio.position = {position} }}",
            feed_name(from),
            plain(from)
        ),
        format!(
            "--playback-props={{ media.class = Audio/Source node.name = {PRESENTED_MICROPHONE}{} \
             node.description = \"{}\" audio.position = {position} }}",
            slug(from),
            microphone_label(from)
        ),
    ]
}

/// The ffmpeg that decodes the share's Ogg Opus from its stdin and plays it
/// into the output `feed`, with a small buffer.
#[must_use]
pub fn sound_play_args(feed: &str) -> Vec<String> {
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-fflags",
        "nobuffer",
        "-flags",
        "low_delay",
        "-f",
        "ogg",
        "-i",
        "pipe:0",
        "-f",
        "pulse",
        "-device",
        feed,
        "-buffer_duration",
        "40",
        "Ligature microphone",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Whether the output `name` exists, by PulseAudio's listing.
fn output_exists(name: &str) -> bool {
    Command::new("pactl")
        .args(["list", "short", "sinks"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .is_some_and(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|line| line.split('\t').nth(1) == Some(name))
        })
}

/// The first `v4l2loopback` device, where the module is loaded.
fn loopback_device() -> Option<PathBuf> {
    let mut nodes: Vec<String> = std::fs::read_dir("/sys/devices/virtual/video4linux")
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with("video"))
        .collect();
    nodes.sort_by_key(|name| name[5..].parse::<u32>().unwrap_or(u32::MAX));
    nodes
        .into_iter()
        .map(|node| Path::new("/dev").join(node))
        .find(|device| device.exists())
}

#[derive(Debug, Default, Clone)]
struct PresentProbe {
    /// ffmpeg decodes H.264.
    decoder: bool,
    /// ffmpeg decodes Opus and plays it into a named output, and PipeWire's
    /// loopback makes that output and its microphone.
    sound: bool,
    /// GStreamer has `pipewiresink` and `rawvideoparse`.
    node: bool,
    /// A `v4l2loopback` device.
    loopback: Option<PathBuf>,
}

/// The Linux presenter. Implements [`Presenter`].
#[derive(Debug, Clone)]
pub struct LinuxPresenter {
    probe: Arc<Mutex<PresentProbe>>,
}

impl LinuxPresenter {
    /// Starts the presenter: probes the tools once and looks for a loopback
    /// device now and then, in the background.
    #[must_use]
    pub fn start() -> Arc<Self> {
        let this = Arc::new(Self {
            probe: Arc::new(Mutex::new(PresentProbe::default())),
        });
        let probe = this.probe.clone();
        std::thread::Builder::new()
            .name("camera-present-probe".into())
            .spawn(move || {
                let decoder = probe_decoder();
                let node = gst_has("pipewiresink") && gst_has("rawvideoparse");
                let sound =
                    sound::probe_player() && sound::runs("pw-loopback") && sound::runs("pactl");
                tracing::info!(decoder, node, sound, "camera presenting tools probed");
                loop {
                    let loopback = loopback_device();
                    {
                        let mut p = lock(&probe);
                        if p.loopback != loopback {
                            tracing::info!(?loopback, "the loopback camera device changed");
                        }
                        *p = PresentProbe {
                            decoder,
                            sound,
                            node,
                            loopback,
                        };
                    }
                    std::thread::sleep(LIST_EVERY);
                }
            })
            .expect("spawn the presenting probe");
        this
    }
}

/// The processes of one presented camera, ended together.
#[derive(Default)]
struct Processes {
    children: Mutex<Vec<Child>>,
    gone: AtomicBool,
}

impl Processes {
    /// Takes `child` in and returns its id, or ends it at once when the
    /// camera has already been taken away.
    fn adopt(&self, mut child: Child) -> Option<u32> {
        let mut children = lock(&self.children);
        if self.gone.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        let id = child.id();
        children.push(child);
        Some(id)
    }

    /// Whether the process `id` is one of these and still runs.
    fn running(&self, id: u32) -> bool {
        lock(&self.children)
            .iter_mut()
            .find(|c| c.id() == id)
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }

    /// Ends the process `id`.
    fn end(&self, id: u32) {
        if let Some(child) = lock(&self.children).iter_mut().find(|c| c.id() == id) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Ends every process, the last started first, so a player ends before
    /// the output it plays into, and takes no more in.
    fn end_all(&self) {
        let mut children = lock(&self.children);
        self.gone.store(true, Ordering::SeqCst);
        for mut child in children.drain(..).rev() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// One process chain a presented share feeds, through a thread of its own
/// so a slow one never holds the share's task.
struct Sink {
    what: &'static str,
    feed: SyncSender<Vec<u8>>,
}

impl Sink {
    /// Starts `first` with its stdin fed by a thread, and `second` reading
    /// `first`'s output where there is one.
    fn start(
        what: &'static str,
        mut first: Command,
        second: Option<Command>,
        processes: &Processes,
    ) -> std::io::Result<Self> {
        first.stdin(Stdio::piped()).stderr(Stdio::inherit());
        first.stdout(if second.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut head = first.spawn()?;
        let stdin = head.stdin.take();
        let piped = head.stdout.take();
        processes.adopt(head);
        let stdin: ChildStdin =
            stdin.ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        if let Some(mut second) = second {
            let piped =
                piped.ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
            processes.adopt(
                second
                    .stdin(Stdio::from(piped))
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()?,
            );
        }
        let (feed, queue) = sync_channel::<Vec<u8>>(SINK_QUEUE);
        std::thread::Builder::new()
            .name("camera-present".into())
            .spawn(move || {
                let mut stdin = stdin;
                for bytes in queue {
                    if stdin.write_all(&bytes).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self { what, feed })
    }

    /// Starts the microphone of the camera from `from`: the loopback pair
    /// now, and on a thread the player once its output is there, fed the
    /// share's Opus packets as Ogg.
    fn microphone(from: &str, channels: u8, processes: Arc<Processes>) -> std::io::Result<Self> {
        let feed_output = feed_name(from);
        if output_exists(&feed_output) {
            // Another camera of the same device is presented with its sound.
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "that device's microphone is presented already",
            ));
        }
        let loopback = Command::new("pw-loopback")
            .args(loopback_args(from, channels))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?;
        let loopback = processes
            .adopt(loopback)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        let (feed, queue) = sync_channel::<Vec<u8>>(SINK_QUEUE);
        std::thread::Builder::new()
            .name("camera-present-sound".into())
            .spawn(move || {
                // The player starts only once its output is there: started
                // sooner, PulseAudio's interface would put the other
                // device's microphone on this machine's default output.
                let deadline = Instant::now() + FEED_WAIT;
                while !output_exists(&feed_output) {
                    if !processes.running(loopback) || Instant::now() > deadline {
                        tracing::warn!(
                            "the microphone's output was not made; presenting without sound"
                        );
                        processes.end(loopback);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                let player = Command::new("ffmpeg")
                    .args(sound_play_args(&feed_output))
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn();
                let mut player = match player {
                    Ok(player) => player,
                    Err(e) => {
                        tracing::warn!(%e, "cannot play the camera's sound; presenting without it");
                        processes.end(loopback);
                        return;
                    }
                };
                let stdin = player.stdin.take();
                let (Some(player), Some(mut stdin)) = (processes.adopt(player), stdin) else {
                    processes.end(loopback);
                    return;
                };
                let mut writer = ogg::Writer::new(std::process::id());
                let mut flowing = stdin.write_all(&writer.headers(channels)).is_ok();
                // What waited while the output was made is old: passed over,
                // so the microphone starts at now.
                while queue.try_recv().is_ok() {}
                while flowing {
                    let Ok(packet) = queue.recv() else { break };
                    // A player without its output would be moved to the
                    // default one: it ends with it.
                    flowing = processes.running(loopback)
                        && stdin.write_all(&writer.packet(&packet)).is_ok();
                }
                drop(stdin);
                processes.end(player);
                processes.end(loopback);
            })?;
        Ok(Self {
            what: "the microphone",
            feed,
        })
    }

    /// Queues `bytes`; false once the chain has ended or is stuck.
    fn send(&self, bytes: Vec<u8>) -> bool {
        match self.feed.try_send(bytes) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                tracing::warn!(sink = self.what, "a presenting sink is stuck; dropping it");
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                tracing::warn!(sink = self.what, "a presenting sink ended");
                false
            }
        }
    }
}

/// A camera being presented on Linux.
struct LinuxPresented {
    video: Vec<Sink>,
    sound: Option<Sink>,
    processes: Arc<Processes>,
}

impl Presented for LinuxPresented {
    fn frame(&mut self, _key: bool, bytes: &[u8]) -> bool {
        self.video.retain(|sink| sink.send(bytes.to_vec()));
        !self.video.is_empty()
    }

    fn audio(&mut self, bytes: &[u8]) -> bool {
        if self
            .sound
            .as_ref()
            .is_some_and(|sink| sink.send(bytes.to_vec()))
        {
            return true;
        }
        self.sound = None;
        false
    }
}

impl Drop for LinuxPresented {
    fn drop(&mut self) {
        // The feeds first, so the threads end; then the processes.
        self.video.clear();
        self.sound = None;
        self.processes.end_all();
        tracing::info!("the presented camera was taken away");
    }
}

impl Presenter for LinuxPresenter {
    fn available(&self) -> bool {
        let probe = lock(&self.probe);
        probe.decoder && (probe.node || probe.loopback.is_some())
    }

    fn start(&self, from: &str, started: &Started) -> Result<Box<dyn Presented>, Error> {
        let probe = lock(&self.probe).clone();
        if !probe.decoder {
            return Err(Error::Io("ffmpeg does not decode H.264 here".to_owned()));
        }
        let mut presented = LinuxPresented {
            video: Vec::new(),
            sound: None,
            processes: Arc::new(Processes::default()),
        };
        let ffmpeg = |args: Vec<String>| {
            let mut c = Command::new("ffmpeg");
            c.args(args);
            c
        };
        let gst = |args: Vec<String>| {
            let mut c = Command::new("gst-launch-1.0");
            c.args(args);
            c
        };
        if probe.node {
            match Sink::start(
                "the PipeWire camera",
                ffmpeg(decode_args(
                    started,
                    NODE_PIXELS,
                    &["-f", "rawvideo", "pipe:1"],
                )),
                Some(gst(camera_node_args(from, started))),
                &presented.processes,
            ) {
                Ok(sink) => presented.video.push(sink),
                Err(e) => tracing::warn!(%e, "cannot present the camera as a PipeWire node"),
            }
        }
        if let Some(device) = &probe.loopback {
            let device = device.display().to_string();
            match Sink::start(
                "the loopback device",
                ffmpeg(decode_args(
                    started,
                    LOOPBACK_PIXELS,
                    &["-f", "v4l2", &device],
                )),
                None,
                &presented.processes,
            ) {
                Ok(sink) => presented.video.push(sink),
                Err(e) => {
                    tracing::warn!(%e, %device, "cannot present the camera on the loopback device")
                }
            }
        }
        if presented.video.is_empty() {
            return Err(Error::Io(
                "the camera cannot be presented: neither a PipeWire camera node nor a loopback device could be made"
                    .to_owned(),
            ));
        }
        if let Some(format) = started.audio.filter(|_| probe.sound) {
            match Sink::microphone(from, format.channels, presented.processes.clone()) {
                Ok(sink) => presented.sound = Some(sink),
                Err(e) => tracing::warn!(%e, "cannot present the camera's sound as a microphone"),
            }
        }
        tracing::info!(
            %from,
            camera = %started.source_id(),
            sinks = ?presented.video.iter().map(|s| s.what).collect::<Vec<_>>(),
            microphone = presented.sound.is_some(),
            "presenting a camera"
        );
        Ok(Box::new(presented))
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            display: "usb_cam".to_owned(),
            kind: SourceKind::Camera,
            format: Format::H264,
            audio: Some(AudioFormat::OPUS_MONO),
            width: 1280,
            height: 720,
            frame_rate: 30,
            viewer: "Pixel 8 Pro".to_owned(),
            cast: false,
        }
    }

    fn started() -> Started {
        Started {
            display: Some("back".to_owned()),
            window: None,
            format: Format::H264,
            audio: Some(AudioFormat::OPUS_MONO),
            width: 1280,
            height: 720,
            frame_rate: 30,
        }
    }

    /// What `pw-dump` prints, cut down to what is read: a laptop's camera
    /// behind libcamera, a USB camera, one that offers only Motion-JPEG, a
    /// camera this daemon presents, and nodes that are not cameras.
    const DUMP: &str = r#"[
      {"id": 31, "type": "PipeWire:Interface:Core", "info": {"props": {"core.name": "pipewire-0"}}},
      {"id": 103, "type": "PipeWire:Interface:Device",
       "info": {"props": {"media.class": "Video/Device", "device.name": "libcamera_device"}}},
      {"id": 125, "type": "PipeWire:Interface:Node", "info": {
        "props": {"media.class": "Video/Source", "media.role": "Camera",
                  "node.name": "libcamera_input.__SB_.PC00.LNK1",
                  "node.description": "Built-in Front Camera",
                  "api.libcamera.location": "front", "priority.session": 800},
        "params": {"EnumFormat": [
          {"mediaType": "video", "mediaSubtype": "raw", "format": "RGBA",
           "size": {"width": 640, "height": 480}},
          {"mediaType": "video", "mediaSubtype": "raw", "format": "RGBA",
           "size": {"width": 1920, "height": 1080}},
          {"mediaType": "video", "mediaSubtype": "raw", "format": "RGBA",
           "size": {"width": 3856, "height": 2176}}]}}},
      {"id": 140, "type": "PipeWire:Interface:Node", "info": {
        "props": {"media.class": "Video/Source", "node.name": "v4l2_input.usb-0_1",
                  "node.nick": "USB camera", "priority.session": 1000},
        "params": {"EnumFormat": [
          {"mediaType": "video", "mediaSubtype": "mjpg",
           "size": {"width": 1920, "height": 1080}, "framerate": {"num": 30, "denom": 1}},
          {"mediaType": "video", "mediaSubtype": "raw", "format": "YUY2",
           "size": {"width": 1280, "height": 720},
           "framerate": {"default": {"num": 10, "denom": 1},
                         "min": {"num": 5, "denom": 1}, "max": {"num": 10, "denom": 1}}},
          {"mediaType": "video", "mediaSubtype": "raw", "format": "YUY2",
           "size": {"width": 1280, "height": 720}, "framerate": {"num": 30000, "denom": 1001}},
          {"mediaType": "video", "mediaSubtype": "raw", "format": "YUY2",
           "size": {"width": 640, "height": 480}, "framerate": {"num": 120, "denom": 1}}]}}},
      {"id": 141, "type": "PipeWire:Interface:Node", "info": {
        "props": {"media.class": "Video/Source", "node.name": "v4l2_input.mjpeg_only"},
        "params": {"EnumFormat": [
          {"mediaType": "video", "mediaSubtype": "mjpg",
           "size": {"width": 1280, "height": 720}}]}}},
      {"id": 147, "type": "PipeWire:Interface:Node", "info": {
        "props": {"media.class": "Video/Source", "media.role": "Camera",
                  "node.name": "ligature_camera.pixel_8_pro",
                  "node.description": "Ligature camera (Pixel 8 Pro)"},
        "params": {"EnumFormat": [
          {"mediaType": "video", "mediaSubtype": "raw", "format": "I420",
           "size": {"width": 1280, "height": 720}}]}}},
      {"id": 150, "type": "PipeWire:Interface:Node", "info": {
        "props": {"media.class": "Video/Source", "node.name": "says_nothing"}}},
      {"id": 160, "type": "PipeWire:Interface:Node", "info": {
        "props": {"media.class": "Audio/Source", "node.name": "alsa_input.mic"}}}
    ]"#;

    #[test]
    fn the_cameras_are_pipewire_s_video_sources_that_offer_raw_pictures() {
        let found = pipewire_cameras(DUMP);
        let ids: Vec<&str> = found.iter().map(|f| f.camera.id.as_str()).collect();
        // The session's preferred camera first; the one that offers only
        // Motion-JPEG and the one presented here are not listed.
        assert_eq!(
            ids,
            [
                "v4l2_input.usb-0_1",
                "libcamera_input.__SB_.PC00.LNK1",
                "says_nothing"
            ]
        );
        let usb = &found[0];
        assert_eq!(usb.camera.name.as_deref(), Some("USB camera"));
        assert_eq!(usb.camera.facing, Facing::External);
        // The largest raw size, not the Motion-JPEG one, and its rates.
        assert_eq!((usb.camera.width, usb.camera.height), (1280, 720));
        assert_eq!(usb.camera.frame_rates, [30, 10]);
        assert_eq!(
            usb.origin,
            Origin::PipeWire {
                target: "v4l2_input.usb-0_1".to_owned()
            }
        );
        let built_in = &found[1];
        assert_eq!(
            built_in.camera.name.as_deref(),
            Some("Built-in Front Camera")
        );
        assert_eq!(built_in.camera.facing, Facing::Front);
        // No larger than 1080p, and 30 a second where no rate is said.
        assert_eq!(
            (built_in.camera.width, built_in.camera.height),
            (1920, 1080)
        );
        assert_eq!(built_in.camera.frame_rates, [UNSTATED_RATE]);
        // A node that says nothing of its formats is taken on trust.
        assert_eq!(
            (found[2].camera.width, found[2].camera.height),
            UNSTATED_SIZE
        );
        assert_eq!(found[2].camera.name, None);

        assert!(pipewire_cameras("").is_empty());
        assert!(pipewire_cameras("{\"not\": \"a list\"}").is_empty());
    }

    #[test]
    fn the_best_size_is_the_largest_within_1080p_or_the_smallest_above_it() {
        let sizes = [(640, 480), (1920, 1080), (3840, 2160), (0, 0)];
        assert_eq!(best_size(sizes.into_iter()), Some((1920, 1080)));
        assert_eq!(
            best_size([(3840, 2160), (2560, 1440)].into_iter()),
            Some((2560, 1440))
        );
        // Taller than 1080 rows does not fit, whatever its area.
        assert_eq!(
            best_size([(1200, 1600), (640, 480)].into_iter()),
            Some((640, 480))
        );
        assert_eq!(best_size(std::iter::empty()), None);
    }

    #[test]
    fn a_v4l2_device_s_size_comes_from_ffmpeg_s_listing() {
        let listing = "\
[video4linux2,v4l2 @ 0x55d0] Raw       :     yuyv422 :           YUYV 4:2:2 : 640x480 320x240 1280x720
[video4linux2,v4l2 @ 0x55d0] Compressed:       mjpeg :          Motion-JPEG : 640x480 1920x1080 2560x1440
[in#0 @ 0x55d0] Error opening input: Immediate exit requested
";
        assert_eq!(v4l2_size(listing), Some((1920, 1080)));
        // A sensor's raw node, which ffmpeg reads nothing from, and a
        // device that is no camera.
        let bayer =
            "[video4linux2,v4l2 @ 0x1] Raw       : Unsupported : 10-bit Bayer BGGR : 3856x2176\n";
        assert_eq!(v4l2_size(bayer), None);
        assert_eq!(v4l2_size("Not a video capture device.\n"), None);
        // A stepwise range of sizes is passed over.
        let stepwise = "[video4linux2,v4l2 @ 0x1] Raw       :     yuv420p : Planar : {32-4096, 2}x{32-2304, 2}\n";
        assert_eq!(v4l2_size(stepwise), None);
    }

    #[test]
    fn a_camera_s_reader_names_its_camera_and_takes_no_other() {
        let args = pipewire_args("v4l2_input.usb-0_1", &settings());
        assert_eq!(args[..2], ["-q", "pipewiresrc"]);
        assert!(args.contains(&"target-object=\"v4l2_input.usb-0_1\"".to_owned()));
        assert!(
            args.contains(
                &"stream-properties=\"props,node.dont-fallback=true,node.dont-reconnect=true\""
                    .to_owned()
            )
        );
        let joined = args.join(" ");
        assert!(joined.contains("! videorate ! videoconvert ! videoscale add-borders=true"));
        assert!(joined.contains(
            "video/x-raw,format=BGRx,width=1280,height=720,framerate=30/1,pixel-aspect-ratio=1/1"
        ));
        // What waits for the daemon is dropped, so the camera is never held up.
        assert!(joined.ends_with(
            "! queue leaky=downstream max-size-buffers=2 max-size-bytes=0 max-size-time=0 \
             ! fdsink fd=1 sync=false"
        ));
        // A name with a quote in it stays one value.
        assert_eq!(quoted("a \"b\" \\c"), "\"a \\\"b\\\" \\\\c\"");

        let v4l2 = v4l2_args(Path::new("/dev/video0"), (1920, 1080), &settings()).join(" ");
        assert!(v4l2.contains("-f v4l2 -framerate 30 -video_size 1920x1080 -i /dev/video0"));
        assert!(v4l2.contains(
            "scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2,fps=30"
        ));
        assert!(v4l2.ends_with("-pix_fmt bgr0 -f rawvideo pipe:1"));
    }

    #[test]
    fn a_presented_camera_is_a_camera_node_and_a_microphone_named_for_the_device() {
        assert_eq!(camera_label("Pixel 8 Pro"), "Ligature camera (Pixel 8 Pro)");
        assert_eq!(
            microphone_label("Garths-MacBook-Pro"),
            "Ligature microphone (Garths-MacBook-Pro)"
        );
        // Nothing that would end the quoted value or the structure.
        assert_eq!(
            camera_label("Garth's \"phone\", the\\new=one"),
            "Ligature camera (Garth s phone the new one)"
        );
        assert_eq!(camera_label("\"\""), "Ligature camera (device)");
        assert_eq!(slug("Pixel 8 Pro"), "pixel_8_pro");

        let node = camera_node_args("Pixel 8 Pro", &started());
        let joined = node.join(" ");
        // A packed picture, two bytes a pixel, so it is one block of memory.
        assert!(joined.starts_with("-q fdsrc fd=0 blocksize=1843200 ! rawvideoparse format=yuy2 width=1280 height=720 framerate=30/1"));
        assert!(joined.contains("! queue leaky=downstream max-size-buffers=2"));
        assert!(joined.contains("! pipewiresink mode=provide sync=false"));
        assert_eq!(
            node.last().map(String::as_str),
            Some(
                "stream-properties=\"props,media.class=Video/Source,media.role=Camera,\
                 node.name=ligature_camera.pixel_8_pro,\
                 node.description=\\\"Ligature camera (Pixel 8 Pro)\\\"\""
            )
        );
        // What is presented here is never listed as a camera to share.
        assert!("ligature_camera.pixel_8_pro".starts_with(PRESENTED_CAMERA));

        let decode = decode_args(&started(), NODE_PIXELS, &["-f", "rawvideo", "pipe:1"]).join(" ");
        assert!(
            decode.contains("-flags low_delay -probesize 32 -analyzeduration 0 -f h264 -i pipe:0")
        );
        assert!(decode.ends_with(
            "-vf scale=1280:720 -pix_fmt yuyv422 -fps_mode passthrough -f rawvideo pipe:1"
        ));
        let loopback =
            decode_args(&started(), LOOPBACK_PIXELS, &["-f", "v4l2", "/dev/video10"]).join(" ");
        assert!(loopback.ends_with("-pix_fmt yuv420p -fps_mode passthrough -f v4l2 /dev/video10"));

        // The sound: an output played into, and the microphone at its other
        // end.
        let pair = loopback_args("Pixel 8 Pro", 1);
        assert_eq!(
            pair,
            [
                "--capture-props={ media.class = Audio/Sink \
                 node.name = ligature_microphone_feed.pixel_8_pro \
                 node.description = \"Ligature microphone feed (Pixel 8 Pro)\" \
                 audio.position = [ MONO ] }",
                "--playback-props={ media.class = Audio/Source \
                 node.name = ligature_microphone.pixel_8_pro \
                 node.description = \"Ligature microphone (Pixel 8 Pro)\" \
                 audio.position = [ MONO ] }",
            ]
        );
        assert!(loopback_args("Mac", 2)[1].ends_with("audio.position = [ FL FR ] }"));
        assert!(
            pair[1].contains(&format!("node.name = {PRESENTED_MICROPHONE}pixel_8_pro")),
            "never captured as this machine's own microphone"
        );
        let play = sound_play_args("ligature_microphone_feed.pixel_8_pro").join(" ");
        assert!(play.contains("-f ogg -i pipe:0"));
        assert!(play.ends_with(
            "-f pulse -device ligature_microphone_feed.pixel_8_pro -buffer_duration 40 Ligature microphone"
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_camera_that_is_not_listed_is_never_started() {
        // Listing one camera that does not exist, this capture reaches no
        // camera of the machine's own, whatever is plugged into it.
        let camera = LinuxCamera::start_with(
            Some("ligature_test.no_such_camera".to_owned()),
            Some("ligature_test.no_such_source".to_owned()),
        );
        assert_eq!(format!("{camera:?}"), "LinuxCamera");
        assert!(camera.displays().is_empty());
        let capture = camera.clone();
        tokio::task::spawn_blocking(move || {
            // Before the probe and after it: nothing to share.
            for _ in 0..2 {
                assert!(capture.cameras().is_empty());
                let params = capture.params();
                assert!(!params.shares && !params.views);
                assert!(params.encodes.iter().all(|f| *f == Format::H264));
                // A display is not this capture's to share.
                let mut display = settings();
                display.kind = SourceKind::Display;
                assert_eq!(capture.start(&display).err(), Some(RefuseReason::NoDisplay));
                // The camera named is not there: refused, for want of an
                // encoder where the machine has none and of the camera
                // where it has one.
                let mut missing = settings();
                missing.display = "ligature_test.no_such_camera".to_owned();
                assert!(matches!(
                    capture.start(&missing).err(),
                    Some(RefuseReason::NoDisplay | RefuseReason::NoEncoder)
                ));
                // Another camera's name is refused the same way.
                assert!(matches!(
                    capture.start(&settings()).err(),
                    Some(RefuseReason::NoDisplay | RefuseReason::NoEncoder)
                ));
                std::thread::sleep(Duration::from_millis(1500));
            }
        })
        .await
        .unwrap();
        assert!(camera.list_now(&[]).is_empty());
    }

    #[test]
    fn a_sink_that_ends_or_is_stuck_is_dropped_and_the_last_one_ends_the_presenting() {
        // `true` reads nothing and ends at once: its feed breaks.
        let processes = Arc::new(Processes::default());
        let ended = Sink::start("ended", Command::new("true"), None, &processes).expect("spawn");
        // `sleep` never reads: its feed fills.
        let mut sleep = Command::new("sleep");
        sleep.arg("30");
        let stuck = Sink::start("stuck", sleep, None, &processes).expect("spawn");
        let ids: Vec<u32> = lock(&processes.children).iter().map(Child::id).collect();
        assert_eq!(ids.len(), 2);
        assert!(processes.running(ids[1]));
        let mut presented = LinuxPresented {
            video: vec![ended, stuck],
            sound: None,
            processes: processes.clone(),
        };
        // A megabyte a frame fills the pipe of a process that reads nothing
        // and then the queue behind it.
        let frame = vec![0u8; 1 << 20];
        let mut alive = true;
        for _ in 0..(SINK_QUEUE * 2) {
            alive = presented.frame(false, &frame);
            if !alive {
                break;
            }
        }
        assert!(!alive, "both sinks should have been dropped");
        assert!(presented.video.is_empty());
        // No sound was agreed: a packet goes nowhere.
        assert!(!presented.audio(&[0u8; 8]));
        // Taken away, its processes end, and one started late ends at once.
        drop(presented);
        assert!(!processes.running(ids[1]));
        assert!(lock(&processes.children).is_empty());
        let late = Command::new("sleep").arg("30").spawn().expect("spawn");
        assert_eq!(processes.adopt(late), None);
    }
}
