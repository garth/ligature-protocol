//! The phone's screen as a share, through the host (milestone 8, LP-151).
//!
//! Android lets only the app capture the screen, after the user consents in
//! the media projection dialog, and only the app runs `MediaCodec`. So the
//! capture asks the host: [`crate::Host::screen_share_requested`] when a
//! viewer starts a share, which the app turns into a notification the user
//! taps to reach the dialog; the app answers with [`HostScreen::started`],
//! then delivers encoded access units with [`HostScreen::frame`] and the end
//! with [`HostScreen::ended`]. A viewer's keyframe request and the stop go
//! back through the host.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lp_core::screen::{Display, Format, RefuseReason, ScreenParams, StopReason};
use tokio::sync::mpsc;

use ligatured::screen::{Capture, Captured, EncodedFrame, Settings, Share, ShareControl};

use crate::SharedHost;

/// How long the user has to answer the projection dialog.
const CONSENT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
struct State {
    available: bool,
    width: u32,
    height: u32,
    scale: f64,
    starting: Option<std::sync::mpsc::Sender<bool>>,
    frames: Option<mpsc::UnboundedSender<Captured>>,
}

/// The phone's screen capture: the app's media projection and encoder.
pub struct HostScreen {
    host: SharedHost,
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for HostScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostScreen")
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl HostScreen {
    /// A capture through `host`; nothing is shared until the host says it
    /// can with [`HostScreen::set_available`].
    #[must_use]
    pub fn new(host: SharedHost) -> Arc<Self> {
        Arc::new(Self {
            host,
            state: Arc::new(Mutex::new(State::default())),
        })
    }

    /// Whether the app has an H.264 encoder, the screen's size, and its
    /// pixels per logical point.
    pub fn set_available(&self, available: bool, width: u32, height: u32, scale: f64) {
        let mut s = lock(&self.state);
        s.available = available;
        s.width = width;
        s.height = height;
        s.scale = scale;
    }

    /// The user answered the projection dialog.
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

    /// The share ended on the phone: `consent` when the user stopped the
    /// projection, `sharer` from the app's own Stop, `error` otherwise.
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

impl Capture for HostScreen {
    fn params(&self) -> ScreenParams {
        let s = lock(&self.state);
        ScreenParams {
            shares: s.available && s.width > 0 && s.height > 0,
            views: false,
            // MediaCodec's encoders give 4:2:0 (ADR 0044).
            encodes: vec![Format::H264],
            ..ScreenParams::default()
        }
    }

    fn displays(&self) -> Vec<Display> {
        let s = lock(&self.state);
        if s.width == 0 || s.height == 0 {
            return Vec::new();
        }
        vec![Display {
            id: "screen".to_owned(),
            name: Some("Phone screen".to_owned()),
            width: s.width,
            height: s.height,
            primary: true,
            scale: (s.scale > 0.0).then_some(s.scale),
        }]
    }

    fn start(&self, settings: &Settings) -> Result<Share, RefuseReason> {
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut s = lock(&self.state);
            if s.frames.is_some() || s.starting.is_some() {
                return Err(RefuseReason::Busy);
            }
            s.starting = Some(tx);
        }
        self.host.screen_share_requested(
            settings.viewer.clone(),
            settings.width,
            settings.height,
            settings.frame_rate,
        );
        let ok = rx.recv_timeout(CONSENT_TIMEOUT).unwrap_or(false);
        lock(&self.state).starting = None;
        if !ok {
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
                    ShareControl::Keyframe => host.screen_share_keyframe(),
                    ShareControl::Stop => break,
                }
            }
            lock(&state).frames = None;
            host.screen_share_stop();
        });
        Ok(Share {
            frames,
            control,
            region: None,
        })
    }
}
