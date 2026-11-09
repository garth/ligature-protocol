//! The phone's cameras as a share, through the host (milestone 10, LP-209).
//!
//! Android lets only the app open a camera and the microphone, and only the
//! app runs `MediaCodec`. So the capture asks the host:
//! [`crate::Host::camera_share_requested`] when a viewer starts a share or
//! the user casts, which the app turns into its camera service; the app
//! answers with [`HostCamera::started`], then delivers encoded access units
//! with [`HostCamera::frame`], the microphone's Opus packets with
//! [`HostCamera::audio`], and the end with [`HostCamera::ended`]. A viewer's
//! keyframe request and the stop go back through the host. The app says
//! which cameras the phone has, and offers none until the user has allowed
//! the camera.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lp_core::camera::{Camera, Facing};
use lp_core::screen::{
    AudioFormat, Display, Format, RefuseReason, ScreenParams, SourceKind, StopReason,
};
use tokio::sync::mpsc;

use ligatured::screen::{
    AudioPacket, Capture, Captured, EncodedFrame, Settings, Share, ShareControl,
};

use crate::SharedHost;

/// How long the app has to start the camera, which may wait on the user
/// tapping its notification; the app is then told to stop asking.
const START_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
struct State {
    /// The app has an H.264 encoder and the user allowed the camera.
    available: bool,
    /// The app has an Opus encoder and may record.
    audio: bool,
    cameras: Vec<Camera>,
    starting: Option<std::sync::mpsc::Sender<bool>>,
    frames: Option<mpsc::UnboundedSender<Captured>>,
}

/// The phone's camera capture: the app's camera service and encoders.
pub struct HostCamera {
    host: SharedHost,
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for HostCamera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostCamera")
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the peers are told of a camera the app listed.
fn listed(info: crate::CameraInfo) -> Camera {
    Camera {
        id: info.id,
        name: Some(info.name).filter(|n| !n.is_empty()),
        facing: match info.facing.as_str() {
            "front" => Facing::Front,
            "back" => Facing::Back,
            _ => Facing::External,
        },
        width: info.width,
        height: info.height,
        frame_rates: info.frame_rates,
    }
}

impl HostCamera {
    /// A capture through `host`; nothing is shared until the host says it
    /// can with [`HostCamera::set_available`].
    #[must_use]
    pub fn new(host: SharedHost) -> Arc<Self> {
        Arc::new(Self {
            host,
            state: Arc::new(Mutex::new(State::default())),
        })
    }

    /// Whether the app can share a camera (it has an H.264 encoder and the
    /// user allowed the camera), whether the microphone can go with it, and
    /// the phone's cameras, the one shared by default first.
    pub fn set_available(&self, available: bool, audio: bool, cameras: Vec<crate::CameraInfo>) {
        let mut s = lock(&self.state);
        s.available = available;
        s.audio = audio;
        s.cameras = cameras
            .into_iter()
            .map(listed)
            .filter(|c| !c.id.is_empty() && c.width > 0 && c.height > 0)
            .collect();
    }

    /// The app started the camera, or could not.
    pub fn started(&self, ok: bool) {
        if let Some(tx) = lock(&self.state).starting.take() {
            let _ = tx.send(ok);
        }
    }

    /// One encoded access unit from the app's encoder.
    pub fn frame(&self, key: bool, bytes: Vec<u8>) {
        if let Some(tx) = lock(&self.state).frames.as_ref() {
            let _ = tx.send(Captured::Frame(EncodedFrame { at: 0, key, bytes }));
        }
    }

    /// One packet of the microphone's sound from the app's encoder: 20 ms of
    /// Opus, stamped by the sync on arrival.
    pub fn audio(&self, bytes: Vec<u8>) {
        if let Some(tx) = lock(&self.state).frames.as_ref() {
            let _ = tx.send(Captured::Audio(AudioPacket { at: 0, bytes }));
        }
    }

    /// The share ended on the phone: `sharer` from the app's own Stop,
    /// `consent` when the permission was taken back, `error` otherwise.
    pub fn ended(&self, reason: &str) {
        let reason = match reason {
            "consent" => StopReason::Consent,
            "sharer" => StopReason::Sharer,
            _ => StopReason::Error,
        };
        if let Some(tx) = lock(&self.state).frames.take() {
            let _ = tx.send(Captured::Ended(reason));
        }
    }
}

impl Capture for HostCamera {
    fn params(&self) -> ScreenParams {
        let s = lock(&self.state);
        ScreenParams {
            shares: s.available && !s.cameras.is_empty(),
            views: false,
            encodes: vec![Format::H264],
            audio_encodes: if s.audio {
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
        lock(&self.state).cameras.clone()
    }

    fn start(&self, settings: &Settings) -> Result<Share, RefuseReason> {
        if settings.kind != SourceKind::Camera {
            return Err(RefuseReason::NoDisplay);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut s = lock(&self.state);
            if !s.cameras.iter().any(|c| c.id == settings.display) {
                return Err(RefuseReason::NoDisplay);
            }
            // One camera at a time: the app runs one camera service.
            if s.frames.is_some() || s.starting.is_some() {
                return Err(RefuseReason::Busy);
            }
            s.starting = Some(tx);
        }
        self.host.camera_share_requested(
            settings.viewer.clone(),
            settings.display.clone(),
            settings.width,
            settings.height,
            settings.frame_rate,
            settings.audio.map_or(0, |a| a.channels),
        );
        let ok = rx.recv_timeout(START_TIMEOUT).unwrap_or(false);
        lock(&self.state).starting = None;
        if !ok {
            // Unanswered or refused: the app withdraws whatever still asks.
            self.host.camera_share_stop();
            return Err(RefuseReason::Consent);
        }
        let (frames_tx, frames) = mpsc::unbounded_channel();
        let (control, mut control_rx) = mpsc::unbounded_channel();
        lock(&self.state).frames = Some(frames_tx);
        let host = self.host.clone();
        let state = self.state.clone();
        tokio::runtime::Handle::current().spawn(async move {
            while let Some(c) = control_rx.recv().await {
                match c {
                    ShareControl::Keyframe => host.camera_share_keyframe(),
                    ShareControl::Stop => break,
                }
            }
            lock(&state).frames = None;
            host.camera_share_stop();
        });
        Ok(Share {
            frames,
            control,
            region: None,
        })
    }
}
