//! The camera capability on macOS (milestone 10, LP-208): the Mac's cameras
//! through AVFoundation, shared with the default microphone, H.264 encoding
//! by ffmpeg's VideoToolbox encoder.
//!
//! AVFoundation hands the daemon BGRA frames at the size and rate the
//! camera gives, and the microphone's samples in the microphone's own
//! format. A pacer thread writes the newest frame into the [`LiveEncoder`]
//! at the agreed rate, and the encoder fits the camera's shape into the
//! agreed picture. The first buffer of sound says the microphone's rate and
//! channels, which is when its Opus encoder starts
//! ([`crate::screen::sound`]).
//!
//! macOS asks for the Camera once, and for the Microphone once, in dialogs
//! that name `Ligature.app` (LP-94): the first share of a camera waits for
//! the answer, and one that is refused is refused for consent. A refused
//! microphone leaves the camera silent. While a camera is captured macOS
//! shows its own indicator, the light beside the camera and the dot in the
//! menu bar, and one for the microphone.
//!
//! The Mac does not present a camera it is sent: a camera offered to macOS
//! programs is a Core Media I/O extension, which macOS loads only from an
//! application signed with a Developer ID and notarised by Apple, and the
//! project's bundle is neither. So this capture has no presenter beside it,
//! and the platform notes say that this is Apple's rule and not a limit of
//! the protocol.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_core::camera::{Camera, Facing};
use lp_core::screen::{
    AudioFormat, Display, Format, RefuseReason, ScreenParams, SourceKind, StopReason,
};
use lp_macos::camera::{self as avf, Access, CameraDevice, Position, StartError};
use tokio::sync::mpsc;

use crate::screen::encode::{Encoder, LiveEncoder, RawInput, probe_encoder};
use crate::screen::sound;
use crate::screen::{Capture, Captured, Settings, Share, ShareControl};

/// How often ffmpeg's encoders and the cameras are looked at again.
const PROBE_EVERY: Duration = Duration::from_secs(10);
/// How long a camera has to give its first picture.
const FIRST_PICTURE: Duration = Duration::from_secs(5);
/// A camera sends a picture at its rate always, so one that has sent
/// nothing for this long has gone: the share ends.
const STALL: Duration = Duration::from_secs(6);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the peers are told of a camera.
fn listed(device: &CameraDevice) -> Camera {
    Camera {
        id: device.id.clone(),
        name: Some(device.name.clone()),
        facing: match device.position {
            Position::Front => Facing::Front,
            Position::Back => Facing::Back,
            Position::Unspecified => Facing::External,
        },
        width: device.width,
        height: device.height,
        frame_rates: device.rates.clone(),
    }
}

#[derive(Debug, Default, Clone)]
struct Probe {
    encoder: Option<Encoder>,
    sound: bool,
    cameras: Vec<CameraDevice>,
}

/// The newest picture the camera gave, and whether the pacer has taken it.
#[derive(Default)]
struct Latest {
    picture: Option<(Vec<u8>, u32, u32)>,
    fresh: bool,
}

/// The Mac's camera capture. Implements [`Capture`] for the camera family;
/// shares while ffmpeg has an H.264 encoder and a camera is connected, with
/// the microphone while ffmpeg has `libopus`.
pub struct MacCamera {
    handle: tokio::runtime::Handle,
    probe: Arc<Mutex<Probe>>,
}

impl std::fmt::Debug for MacCamera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacCamera")
    }
}

impl MacCamera {
    /// Starts looking for ffmpeg's encoders and the cameras in the
    /// background. Must be called within the daemon's runtime.
    #[must_use]
    pub fn start() -> Arc<Self> {
        let this = Arc::new(Self {
            handle: tokio::runtime::Handle::current(),
            probe: Arc::new(Mutex::new(Probe::default())),
        });
        let probe = this.probe.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("camera-probe".into())
            .spawn(move || {
                let mut round: u32 = 0;
                let mut tools = (None, false);
                loop {
                    // The tools once a minute, the cameras every round.
                    if round.is_multiple_of(6) {
                        tools = (probe_encoder(), sound::probe_encoder());
                    }
                    round = round.wrapping_add(1);
                    let cameras = avf::cameras();
                    {
                        let mut p = lock(&probe);
                        if p.cameras != cameras || p.encoder != tools.0 || p.sound != tools.1 {
                            tracing::info!(
                                encoder = ?tools.0,
                                sound = tools.1,
                                cameras = ?cameras.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
                                "camera sharing probed"
                            );
                        }
                        *p = Probe {
                            encoder: tools.0,
                            sound: tools.1,
                            cameras,
                        };
                    }
                    std::thread::sleep(PROBE_EVERY);
                }
            })
        {
            tracing::warn!(%e, "cannot start the camera probe");
        }
        this
    }
}

impl Capture for MacCamera {
    fn params(&self) -> ScreenParams {
        let probe = lock(&self.probe);
        // A refusal in System Settings is known without asking; a camera
        // never asked for is offered, and its first share asks.
        let shares = probe.encoder.is_some()
            && !probe.cameras.is_empty()
            && avf::camera_access() != Access::Denied;
        ScreenParams {
            shares,
            views: false,
            encodes: if probe.encoder.is_some() {
                vec![Format::H264]
            } else {
                Vec::new()
            },
            audio_encodes: if probe.sound && avf::microphone_access() != Access::Denied {
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
        lock(&self.probe).cameras.iter().map(listed).collect()
    }

    fn start(&self, settings: &Settings) -> Result<Share, RefuseReason> {
        if settings.kind != SourceKind::Camera {
            return Err(RefuseReason::NoDisplay);
        }
        let probe = lock(&self.probe).clone();
        let encoder = probe.encoder.ok_or(RefuseReason::NoEncoder)?;
        let (frames_tx, frames) = mpsc::unbounded_channel::<Captured>();

        let latest = Arc::new(Mutex::new(Latest::default()));
        let on_frame = {
            let latest = latest.clone();
            Box::new(move |pixels: &[u8], width: u32, height: u32| {
                let mut latest = lock(&latest);
                latest.picture = Some((pixels.to_vec(), width, height));
                latest.fresh = true;
            }) as avf::OnFrame
        };
        // The microphone, when agreed: its encoder starts with the first
        // buffer, which says the microphone's rate and channels; one that
        // cannot start leaves the camera silent.
        let on_sound = settings.audio.filter(|_| probe.sound).map(|format| {
            let frames_tx = frames_tx.clone();
            let encoder: Mutex<Option<Option<(sound::Sound, std::process::ChildStdin)>>> =
                Mutex::new(None);
            Box::new(move |samples: &[f32], rate: u32, channels: u8| {
                use std::io::Write as _;
                let mut guard = lock(&encoder);
                let slot = guard.get_or_insert_with(|| {
                    sound::Sound::start_pcm_from(rate, channels, format, frames_tx.clone())
                        .inspect_err(
                            |e| tracing::warn!(%e, "cannot encode the microphone; sharing silent"),
                        )
                        .ok()
                });
                let Some((_, pipe)) = slot.as_mut() else {
                    return;
                };
                let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                if pipe.write_all(&bytes).is_err() {
                    *slot = None;
                }
            }) as avf::OnSound
        });
        let stream =
            avf::camera_stream(&settings.display, on_frame, on_sound).map_err(|e| match e {
                StartError::Consent => {
                    tracing::info!(
                        "sharing a camera needs Ligature allowed under Camera in System Settings, \
                         Privacy and Security"
                    );
                    RefuseReason::Consent
                }
                StartError::NoCamera => RefuseReason::NoDisplay,
                StartError::Failed(text) => {
                    tracing::warn!(%text, "AVFoundation refused the camera");
                    RefuseReason::NoEncoder
                }
            })?;

        // The first picture says the size the camera gives.
        let waited = Instant::now();
        let first = loop {
            if let Some((_, width, height)) = lock(&latest).picture.as_ref() {
                break Some((*width, *height));
            }
            if waited.elapsed() > FIRST_PICTURE {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let Some((width, height)) = first else {
            tracing::warn!("the camera gave no picture; refusing the share");
            return Err(RefuseReason::NoDisplay);
        };
        let input = RawInput {
            width,
            height,
            pix_fmt: "bgra".to_owned(),
        };
        let live = LiveEncoder::start(
            encoder,
            input.clone(),
            settings,
            frames_tx.clone(),
            Arc::new(Mutex::new(Instant::now())),
        )?;
        tracing::info!(
            camera = %settings.display,
            ?encoder,
            microphone = stream.microphone,
            from = format!("{width}x{height}"),
            size = format!("{}x{}", settings.width, settings.height),
            rate = settings.frame_rate,
            "camera capture started"
        );

        // The pacer: the newest picture into the encoder at the agreed
        // rate, the encoder started again if the camera changes its size,
        // and the share ended when the camera stops giving pictures.
        let stopping = Arc::new(AtomicBool::new(false));
        {
            let (stopping, latest, pacer_live, frames_tx) = (
                stopping.clone(),
                latest.clone(),
                live.clone(),
                frames_tx.clone(),
            );
            let period = Duration::from_secs(1) / settings.frame_rate.max(1);
            let spawned = std::thread::Builder::new()
                .name("camera-pacer".into())
                .spawn(move || {
                    let mut input = input;
                    let mut next = Instant::now();
                    let mut last_picture = Instant::now();
                    while !stopping.load(Ordering::SeqCst) {
                        let picture = {
                            let mut latest = lock(&latest);
                            if std::mem::take(&mut latest.fresh) {
                                latest.picture.clone()
                            } else {
                                None
                            }
                        };
                        if let Some((pixels, width, height)) = picture {
                            last_picture = Instant::now();
                            if (width, height) != (input.width, input.height) {
                                input = RawInput {
                                    width,
                                    height,
                                    pix_fmt: "bgra".to_owned(),
                                };
                                if pacer_live.restart(input.clone()).is_err() {
                                    break;
                                }
                            }
                            if pacer_live.write(&pixels).is_err() {
                                break;
                            }
                        } else if last_picture.elapsed() > STALL {
                            tracing::warn!("the camera sent nothing; ending the share");
                            let _ = frames_tx.send(Captured::Ended(StopReason::Error));
                            break;
                        }
                        next += period;
                        let now = Instant::now();
                        if next > now {
                            std::thread::sleep(next - now);
                        } else {
                            next = now;
                        }
                    }
                });
            if let Err(e) = spawned {
                tracing::warn!(%e, "cannot start the camera pacer");
                live.stop();
                return Err(RefuseReason::NoEncoder);
            }
        }

        // Control: a keyframe from the encoder started again; a stop ends
        // the capture, which turns the camera's light off, and the encoders.
        let (control, mut control_rx) = mpsc::unbounded_channel::<ShareControl>();
        let mut stream = Some(stream);
        self.handle.spawn(async move {
            while let Some(c) = control_rx.recv().await {
                match c {
                    ShareControl::Keyframe => live.keyframe(),
                    ShareControl::Stop => break,
                }
            }
            stopping.store(true, Ordering::SeqCst);
            let stream = stream.take();
            let _ = tokio::task::spawn_blocking(move || {
                // The microphone's encoder lives in the capture's delegate
                // and ends with it.
                drop(stream);
                live.stop();
            })
            .await;
            tracing::info!("camera capture stopped");
        });

        Ok(Share {
            frames,
            control,
            region: None,
        })
    }
}
