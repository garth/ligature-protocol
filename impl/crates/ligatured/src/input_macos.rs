//! The input capability on macOS (milestone 7, LP-144): capture through a
//! Quartz event tap, injection by posting Quartz events. Both run in the
//! daemon, which `Ligature.app` (LP-94) gives the identity macOS grants the
//! two permissions to.
//!
//! **Capture** ([`MacCapture`]). An event tap ahead of every application
//! sees the keyboard and mouse. While nothing is driven it only watches the
//! cursor, and reports an edge hit when the cursor arrives on an edge the
//! layout maps. While driving it swallows everything, freezes and hides the
//! cursor (the mouse still reports its motion), and turns what it sees into
//! the wire's events: virtual key codes through the table in
//! [`crate::input::mac`], modifier flag changes into presses and releases,
//! repeats dropped since the driven device repeats for itself. A supervisor
//! thread starts the tap once Accessibility and Input Monitoring are both
//! granted, re-reads the displays, and stops the tap if a permission is
//! withdrawn, ending a drive as the compositor's `Lost` does on Linux.
//!
//! **Injection** ([`MacInject`]). Events are posted at the HID level from
//! an event source tagged so this Mac's own tap passes them by. The pointer
//! is tracked here and kept on a display; a button carries the click count
//! a double click needs; a key carries the held modifiers' flags; `text` is
//! typed as key events with the string attached, whatever the layout. The
//! family accepts only while the Accessibility permission is held, which the
//! manifest re-reads every second, so withdrawing it drops the family from
//! every peer's view.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_core::Error;
use lp_core::input::{Edge, InputEvent, InputParams};
use lp_macos::input::{self as quartz, Mouse, TapEvent};
use tokio::sync::broadcast;

use crate::input::geometry::{EdgeWatch, Zone, back_position, clamp_to_zones, point_at, size};
use crate::input::mac::{
    ClickCounter, ModifierCapture, ModifierInject, ScrollUnits, evdev_button, evdev_of, mac_button,
    mac_of, wire_scroll,
};
use crate::input::{Capture, CaptureEvent, Inject};

/// How often the supervisor re-reads the permissions and the displays.
const SUPERVISE: Duration = Duration::from_secs(2);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The displays as zones, in points.
fn zones() -> Vec<Zone> {
    quartz::displays()
        .iter()
        .map(|d| Zone {
            x: d.x.round() as i32,
            y: d.y.round() as i32,
            width: d.width.round().max(0.0) as u32,
            height: d.height.round().max(0.0) as u32,
        })
        .collect()
}

#[derive(Default)]
struct TapState {
    armed: Vec<Edge>,
    zones: Vec<Zone>,
    captured: bool,
    watch: EdgeWatch,
    modifiers: ModifierCapture,
}

/// The Mac's capture: an event tap. Implements [`Capture`]; `provides`
/// while the tap runs.
pub struct MacCapture {
    events: broadcast::Sender<CaptureEvent>,
    state: Arc<Mutex<TapState>>,
    available: Arc<AtomicBool>,
}

impl std::fmt::Debug for MacCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacCapture")
    }
}

impl MacCapture {
    /// Asks for the permissions if they are missing and starts the
    /// supervisor, which starts the tap once they are granted.
    #[must_use]
    pub fn start() -> Arc<Self> {
        quartz::request_access();
        let this = Arc::new(Self {
            events: broadcast::channel(1024).0,
            state: Arc::new(Mutex::new(TapState {
                zones: zones(),
                ..TapState::default()
            })),
            available: Arc::new(AtomicBool::new(false)),
        });
        let (state, available, events) = (
            this.state.clone(),
            this.available.clone(),
            this.events.clone(),
        );
        if let Err(e) = std::thread::Builder::new()
            .name("ligature-input-supervisor".to_owned())
            .spawn(move || supervise(&state, &available, &events))
        {
            tracing::warn!(%e, "cannot start the input capture supervisor");
        }
        this
    }
}

/// Ends a capture from this side: the cursor is let go and the sync told.
fn let_go(state: &Mutex<TapState>, events: &broadcast::Sender<CaptureEvent>) {
    let was = std::mem::replace(&mut lock(state).captured, false);
    if was {
        quartz::hold_cursor(false);
        let _ = events.send(CaptureEvent::Lost);
    }
}

fn supervise(
    state: &Arc<Mutex<TapState>>,
    available: &AtomicBool,
    events: &broadcast::Sender<CaptureEvent>,
) {
    let mut tap: Option<quartz::Tap> = None;
    let mut told = false;
    loop {
        lock(state).zones = zones();
        let allowed = quartz::listen_access() && quartz::post_access();
        if allowed && tap.is_none() {
            match quartz::tap(handler(state.clone(), events.clone())) {
                Ok(t) => {
                    tracing::info!("input capture: the event tap is running");
                    tap = Some(t);
                    told = false;
                }
                Err(e) if !told => {
                    tracing::warn!(%e, "cannot start the input capture");
                    told = true;
                }
                Err(_) => {}
            }
        } else if !allowed && tap.is_some() {
            tap = None;
            let_go(state, events);
            tracing::warn!(
                "the Accessibility or Input Monitoring permission was withdrawn: input capture stopped"
            );
        } else if !allowed && !told {
            tracing::info!(
                "input capture and injection wait for Ligature to be allowed under Accessibility \
                 and Input Monitoring in System Settings, Privacy and Security"
            );
            told = true;
        }
        available.store(tap.is_some(), Ordering::SeqCst);
        std::thread::sleep(SUPERVISE);
    }
}

/// The tap's handler: edge hits while free, every event captured and
/// swallowed while driving.
fn handler(
    state: Arc<Mutex<TapState>>,
    events: broadcast::Sender<CaptureEvent>,
) -> impl FnMut(TapEvent) -> bool + Send + 'static {
    move |event| {
        let mut guard = lock(&state);
        let s = &mut *guard;
        if !s.captured {
            if let TapEvent::Moved { at, .. } = event
                && let Some((edge, at)) = s.watch.update(&s.zones, &s.armed, at)
            {
                drop(guard);
                let _ = events.send(CaptureEvent::EdgeHit { edge, at });
            }
            return false;
        }
        let captured: Vec<InputEvent> = match event {
            TapEvent::Moved { dx, dy, .. } if dx != 0.0 || dy != 0.0 => {
                vec![InputEvent::Motion { dx, dy }]
            }
            TapEvent::Moved { .. } | TapEvent::Disabled => Vec::new(),
            TapEvent::Button { button, pressed } => evdev_button(button)
                .map(|code| InputEvent::Button { code, pressed })
                .into_iter()
                .collect(),
            TapEvent::Scroll {
                continuous,
                lines,
                points,
            } => match wire_scroll(continuous, lines, points) {
                InputEvent::Scroll { dx, dy, .. } if dx == 0.0 && dy == 0.0 => Vec::new(),
                scroll => vec![scroll],
            },
            TapEvent::Key { repeat: true, .. } => Vec::new(),
            TapEvent::Key { code, pressed, .. } => match evdev_of(code) {
                Some(code) => vec![InputEvent::Key { code, pressed }],
                None => {
                    tracing::debug!(code, "a key with no evdev code, dropped");
                    Vec::new()
                }
            },
            TapEvent::Flags { code, flags } => s
                .modifiers
                .changed(code, flags)
                .into_iter()
                .map(|(code, pressed)| InputEvent::Key { code, pressed })
                .collect(),
        };
        drop(guard);
        for e in captured {
            let _ = events.send(CaptureEvent::Input(e));
        }
        true
    }
}

impl Capture for MacCapture {
    fn provides(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    fn screen(&self) -> (u32, u32) {
        size(&lock(&self.state).zones)
    }

    fn arm(&self, edges: &[Edge]) {
        lock(&self.state).armed = edges.to_vec();
    }

    fn take(&self, _edge: Edge) -> Result<(), Error> {
        if !self.provides() {
            return Err(Error::Io("the event tap is not running".to_owned()));
        }
        let mut s = lock(&self.state);
        if !s.captured {
            s.captured = true;
            s.modifiers.clear();
            quartz::hold_cursor(true);
        }
        Ok(())
    }

    fn release(&self, back_at: Option<(Edge, f64)>) {
        let mut s = lock(&self.state);
        if let Some((edge, at)) = back_at {
            quartz::warp(back_position(edge, at, &s.zones));
        }
        if std::mem::replace(&mut s.captured, false) {
            // Reconnecting the mouse after the warp also lifts the pause
            // macOS puts on mouse events after a warp.
            quartz::hold_cursor(false);
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<CaptureEvent> {
        self.events.subscribe()
    }
}

struct InjectState {
    pos: (f64, f64),
    zones: Vec<Zone>,
    buttons: Vec<u16>,
    keys: HashSet<u16>,
    modifiers: ModifierInject,
    clicks: ClickCounter,
    lines: ScrollUnits,
    pixels: ScrollUnits,
}

/// The Mac's injector: Quartz events from a tagged source. Implements
/// [`Inject`]; accepts while the Accessibility permission is held.
pub struct MacInject {
    poster: Option<quartz::Poster>,
    state: Mutex<InjectState>,
}

impl std::fmt::Debug for MacInject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacInject")
    }
}

impl MacInject {
    /// An injector; it posts nothing until a drive begins.
    #[must_use]
    pub fn start() -> Arc<Self> {
        let poster = quartz::Poster::new();
        if poster.is_none() {
            tracing::warn!("no Quartz event source: this Mac cannot be driven");
        }
        Arc::new(Self {
            poster,
            state: Mutex::new(InjectState {
                pos: quartz::cursor(),
                zones: zones(),
                buttons: Vec::new(),
                keys: HashSet::new(),
                modifiers: ModifierInject::default(),
                clicks: ClickCounter::new(quartz::double_click_interval()),
                lines: ScrollUnits::default(),
                pixels: ScrollUnits::default(),
            }),
        })
    }

    fn poster(&self) -> Result<&quartz::Poster, Error> {
        self.poster
            .as_ref()
            .ok_or_else(|| Error::Io("no Quartz event source".to_owned()))
    }

    /// Moves the tracked pointer to `to` (kept on a display) and posts the
    /// move, as a drag while a button is down.
    fn move_to(&self, s: &mut InjectState, to: (f64, f64)) -> Result<(), Error> {
        let poster = self.poster()?;
        let to = clamp_to_zones(&s.zones, to);
        let delta = (to.0 - s.pos.0, to.1 - s.pos.1);
        s.pos = to;
        s.clicks.moved(to);
        let kind = match s.buttons.first().and_then(|b| mac_button(*b)) {
            Some(b) => Mouse::Dragged(b),
            None => Mouse::Moved,
        };
        poster.mouse(kind, to, delta, 0, s.modifiers.flags());
        Ok(())
    }
}

impl Inject for MacInject {
    fn params(&self) -> InputParams {
        let accepts = self.poster.is_some() && quartz::post_access();
        InputParams {
            accepts,
            provides: false,
            absolute: accepts,
            text: accepts,
        }
    }

    fn screen(&self) -> (u32, u32) {
        let mut s = lock(&self.state);
        s.zones = zones();
        size(&s.zones)
    }

    fn begin(&self, start: (f64, f64)) -> Result<(), Error> {
        let mut s = lock(&self.state);
        s.zones = zones();
        let to = clamp_to_zones(&s.zones, point_at(&s.zones, start));
        quartz::warp(to);
        s.pos = to;
        s.lines.clear();
        s.pixels.clear();
        self.move_to(&mut s, to)
    }

    fn inject(&self, event: &InputEvent) -> Result<(), Error> {
        let poster = self.poster()?;
        let mut guard = lock(&self.state);
        let s = &mut *guard;
        match event {
            InputEvent::Motion { dx, dy } => {
                let to = (s.pos.0 + dx, s.pos.1 + dy);
                self.move_to(s, to)?;
            }
            InputEvent::Position { x, y } => {
                let to = point_at(&s.zones, (*x, *y));
                self.move_to(s, to)?;
            }
            InputEvent::Button { code, pressed } => {
                let Some(button) = mac_button(*code) else {
                    tracing::debug!(code, "a button the Mac has no number for, dropped");
                    return Ok(());
                };
                let clicks = if *pressed {
                    if !s.buttons.contains(code) {
                        s.buttons.push(*code);
                    }
                    s.clicks.press(*code, s.pos, Instant::now())
                } else {
                    s.buttons.retain(|b| b != code);
                    s.clicks.current()
                };
                let kind = if *pressed {
                    Mouse::Down(button)
                } else {
                    Mouse::Up(button)
                };
                poster.mouse(kind, s.pos, (0.0, 0.0), clicks, s.modifiers.flags());
            }
            InputEvent::Scroll { dx, dy, discrete } => {
                let units = if *discrete {
                    &mut s.lines
                } else {
                    &mut s.pixels
                };
                let (vertical, horizontal) = units.take(*dx, *dy);
                poster.scroll(*discrete, vertical, horizontal, s.modifiers.flags());
            }
            InputEvent::Key { code, pressed } => {
                let Some(mac) = mac_of(*code) else {
                    tracing::debug!(code, "a key the Mac has no code for, dropped");
                    return Ok(());
                };
                let modifier = s.modifiers.key(*code, *pressed);
                if !modifier {
                    if *pressed {
                        s.keys.insert(*code);
                    } else {
                        s.keys.remove(code);
                    }
                }
                poster.key(mac, *pressed, modifier, s.modifiers.flags());
            }
            InputEvent::Text { text } => poster.text(text),
        }
        Ok(())
    }

    fn end(&self) {
        let Ok(poster) = self.poster() else { return };
        let mut guard = lock(&self.state);
        let s = &mut *guard;
        for code in std::mem::take(&mut s.buttons) {
            if let Some(button) = mac_button(code) {
                poster.mouse(Mouse::Up(button), s.pos, (0.0, 0.0), 1, s.modifiers.flags());
            }
        }
        let mut keys: Vec<u16> = s.keys.drain().collect();
        keys.sort_unstable();
        for code in keys {
            if let Some(mac) = mac_of(code) {
                poster.key(mac, false, false, s.modifiers.flags());
            }
        }
        for code in s.modifiers.held() {
            s.modifiers.key(code, false);
            if let Some(mac) = mac_of(code) {
                poster.key(mac, false, true, s.modifiers.flags());
            }
        }
    }
}
