//! The screen capability on macOS (milestone 8, LP-150): capture through
//! ScreenCaptureKit, H.264 encoding by ffmpeg's VideoToolbox encoder.
//!
//! ScreenCaptureKit hands the daemon BGRA frames of the share's size, the
//! cursor drawn in, but only when something on the display changed. A pacer
//! thread writes each new frame into the encoder, at most at the agreed rate,
//! and the last one again once a second while the screen is still, so the
//! stream keeps going without the viewer decoding the full rate of an
//! unchanged screen. The encoder is the same [`LiveEncoder`] as on Linux: a
//! keyframe at the start and when one is asked for, the last picture of a
//! change settled, and its output split into frames
//! ([`crate::screen::encode`]).
//!
//! ScreenCaptureKit serves only a process with the Screen Recording
//! permission, which macOS grants to `Ligature.app` (LP-94). The first share
//! asks for it and is refused for consent; macOS applies the permission to
//! the next process, so the daemon is restarted once after allowing it. The
//! menu bar's sharing indicator is the platform's own, and stopping the
//! share there ends it with `consent`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_core::input::Region;
use lp_core::screen::{Codec, Display, RefuseReason, ScreenParams, StopReason};
use lp_macos::screen::{self as sck, Ended, ScreenDisplay, StartError};
use tokio::sync::mpsc;

use crate::input::geometry::{Zone, region_of};
use crate::screen::encode::{Encoder, LiveEncoder, RawInput, probe_encoder};
use crate::screen::{Capture, Captured, Settings, Share, ShareControl};

/// How often ffmpeg's encoders are looked at again, for an ffmpeg
/// installed while the daemon runs.
const PROBE_EVERY: Duration = Duration::from_secs(60);
/// How often the pacer writes the last frame again while the screen is still.
const STILL_REPEAT: Duration = Duration::from_secs(1);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn zone(d: &ScreenDisplay) -> Zone {
    Zone {
        x: d.x.round() as i32,
        y: d.y.round() as i32,
        width: d.width.round().max(0.0) as u32,
        height: d.height.round().max(0.0) as u32,
    }
}

fn name(d: &ScreenDisplay) -> String {
    if d.builtin {
        "Built-in display".to_owned()
    } else if d.main {
        "Main display".to_owned()
    } else {
        format!("Display {}", d.id)
    }
}

/// A running share: the encoder and the capture, ended together.
struct Running {
    encoder: Arc<LiveEncoder>,
    stream: Option<sck::Stream>,
}

/// The Mac's screen capture. Implements [`Capture`]; shares while ffmpeg
/// has an H.264 encoder.
pub struct MacScreen {
    handle: tokio::runtime::Handle,
    encoder: Arc<Mutex<Option<Encoder>>>,
}

impl std::fmt::Debug for MacScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacScreen")
    }
}

impl MacScreen {
    /// Starts looking for ffmpeg's encoder in the background. Must be called
    /// within the daemon's runtime.
    #[must_use]
    pub fn start() -> Arc<Self> {
        let this = Arc::new(Self {
            handle: tokio::runtime::Handle::current(),
            encoder: Arc::new(Mutex::new(None)),
        });
        let encoder = this.encoder.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("screen-probe".into())
            .spawn(move || {
                let mut last = None;
                loop {
                    let found = probe_encoder();
                    if found != last {
                        tracing::info!(encoder = ?found, "screen sharing encoder probed");
                        last = found;
                    }
                    *lock(&encoder) = found;
                    std::thread::sleep(PROBE_EVERY);
                }
            })
        {
            tracing::warn!(%e, "cannot start the screen encoder probe");
        }
        this
    }
}

impl Capture for MacScreen {
    fn params(&self) -> ScreenParams {
        ScreenParams {
            shares: lock(&self.encoder).is_some() && !sck::screen_displays().is_empty(),
            views: false,
            codecs: vec![Codec::H264],
            ..ScreenParams::default()
        }
    }

    fn displays(&self) -> Vec<Display> {
        sck::screen_displays()
            .iter()
            .map(|d| Display {
                id: d.id.to_string(),
                name: Some(name(d)),
                width: d.pixel_width,
                height: d.pixel_height,
                primary: d.main,
                // Pixels per point: 2 on a Retina display (ADR 0045).
                scale: (d.width > 0.0).then(|| f64::from(d.pixel_width) / d.width),
            })
            .collect()
    }

    fn region(&self, display: &str) -> Option<Region> {
        let displays = sck::screen_displays();
        let zones: Vec<Zone> = displays.iter().map(zone).collect();
        let shared = displays.iter().find(|d| d.id.to_string() == display)?;
        if displays.len() < 2 {
            return None;
        }
        region_of(&zones, &zone(shared))
    }

    fn start(&self, settings: &Settings) -> Result<Share, RefuseReason> {
        let encoder = (*lock(&self.encoder)).ok_or(RefuseReason::NoEncoder)?;
        let id: u32 = settings
            .display
            .parse()
            .map_err(|_| RefuseReason::NoDisplay)?;
        if !sck::screen_access() {
            sck::request_screen_access();
            tracing::info!(
                "screen sharing needs Ligature allowed under Screen Recording in System Settings, \
                 Privacy and Security; restart the daemon after allowing it"
            );
            return Err(RefuseReason::Consent);
        }
        let input = RawInput {
            width: settings.width,
            height: settings.height,
            pix_fmt: "bgra".to_owned(),
        };
        let (frames_tx, frames) = mpsc::unbounded_channel::<Captured>();
        let live = LiveEncoder::start(
            encoder,
            input,
            settings,
            frames_tx.clone(),
            Arc::new(Mutex::new(Instant::now())),
        )?;
        let (control, mut control_rx) = mpsc::unbounded_channel::<ShareControl>();
        let stopping = Arc::new(AtomicBool::new(false));
        let latest: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        // A picture has come since the pacer last wrote one.
        let fresh = Arc::new(AtomicBool::new(false));
        let frame_bytes = settings.width as usize * settings.height as usize * 4;

        let on_frame = {
            let (latest, fresh) = (latest.clone(), fresh.clone());
            let (w, h) = (settings.width, settings.height);
            Box::new(move |pixels: &[u8], width: u32, height: u32| {
                if (width, height) == (w, h) {
                    *lock(&latest) = Some(pixels.to_vec());
                    fresh.store(true, Ordering::SeqCst);
                }
            }) as sck::OnFrame
        };
        let on_end = {
            let frames_tx = frames_tx.clone();
            let stopping = stopping.clone();
            Box::new(move |ended: Ended| {
                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                let reason = match ended {
                    Ended::User => StopReason::Consent,
                    Ended::Failed(e) => {
                        tracing::warn!(%e, "ScreenCaptureKit ended the share");
                        StopReason::Error
                    }
                };
                let _ = frames_tx.send(Captured::Ended(reason));
            }) as sck::OnEnd
        };
        let stream = match sck::stream(
            id,
            settings.width,
            settings.height,
            settings.frame_rate,
            on_frame,
            on_end,
        ) {
            Ok(s) => s,
            Err(e) => {
                live.stop();
                return Err(match e {
                    StartError::Consent => RefuseReason::Consent,
                    StartError::NoDisplay => RefuseReason::NoDisplay,
                    StartError::Failed(text) => {
                        tracing::warn!(%text, "ScreenCaptureKit refused the share");
                        RefuseReason::NoEncoder
                    }
                });
            }
        };
        tracing::info!(
            display = %settings.display,
            ?encoder,
            size = format!("{}x{}", settings.width, settings.height),
            "screen capture started"
        );

        // The pacer: a new picture into the encoder at most at the agreed
        // rate, and the last one again once a second while the screen is
        // still.
        {
            let stopping = stopping.clone();
            let latest = latest.clone();
            let pacer_live = live.clone();
            let period = Duration::from_secs(1) / settings.frame_rate.max(1);
            let spawned = std::thread::Builder::new()
                .name("screen-pacer".into())
                .spawn(move || {
                    let mut next = Instant::now();
                    let mut written: Option<Instant> = None;
                    while !stopping.load(Ordering::SeqCst) {
                        let due = fresh.swap(false, Ordering::SeqCst)
                            || written.is_none_or(|at| at.elapsed() >= STILL_REPEAT);
                        let frame = if due { lock(&latest).clone() } else { None };
                        if let Some(frame) = frame.filter(|f| f.len() == frame_bytes) {
                            if pacer_live.write(&frame).is_err() {
                                break;
                            }
                            written = Some(Instant::now());
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
                tracing::warn!(%e, "cannot start the screen pacer");
                live.stop();
                return Err(RefuseReason::NoEncoder);
            }
        }

        let running = Arc::new(Mutex::new(Running {
            encoder: live,
            stream: Some(stream),
        }));
        // Control: a keyframe from the encoder started again; a stop ends the
        // capture, the pacer and the encoder.
        self.handle.spawn(async move {
            while let Some(c) = control_rx.recv().await {
                match c {
                    ShareControl::Keyframe => lock(&running).encoder.keyframe(),
                    ShareControl::Stop => break,
                }
            }
            stopping.store(true, Ordering::SeqCst);
            let _ = tokio::task::spawn_blocking(move || {
                let mut r = lock(&running);
                drop(r.stream.take());
                r.encoder.stop();
            })
            .await;
            tracing::info!("screen capture stopped");
        });

        Ok(Share {
            frames,
            control,
            region: self.region(&settings.display),
        })
    }
}
