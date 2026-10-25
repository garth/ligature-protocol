//! `lpview`: watch another of your devices' screens in a window, and drive it
//! (milestone 8, LP-149).
//!
//! The local daemon does the protocol: `lpview` asks it over the control
//! socket to watch a device (`screen.start`) and receives the encoded frames
//! as events, which an ffmpeg child decodes into raw pictures this program
//! draws into a window, scaled to fit. A click on the picture asks the daemon
//! to drive the device (`input.drive`, an absolute entry where the click
//! landed); from then on the pointer's position over the picture, its
//! buttons and wheel, and the keyboard go to the device as `input` events,
//! until both Control keys are pressed, the window loses focus, or it
//! closes. A key travels as its Linux evdev code, from winit's physical key
//! code, and a press carries what it means from winit's logical key (ADR
//! 0043), so the device types what this keyboard's layout says.

use std::io::{Read, Write as _};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;
use ligatured::control::{
    Caps, Client, Devices, Empty, Event, InputDriven, Request, ScreenDisplays, ScreenStopped, types,
};
use lp_core::input::keyid::{self, KeyId};
use lp_core::input::{
    BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, InputEvent, InputParams, KEY_LEFTCTRL, KEY_RIGHTCTRL,
};
use lp_core::paths::{DATA_DIR_ENV, Locations, RUNTIME_DIR_ENV};
use lp_core::screen::{Display, Format, Started};
use lp_core::{Error, PublicKey};
use tokio::sync::mpsc;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement as _;
use winit::window::{Window, WindowId};

/// Command-line options.
#[derive(Debug, Parser)]
#[command(version, about = "Watch and drive another of your devices' screens")]
struct Options {
    /// The device: its name, or a prefix of its key or fingerprint.
    device: String,
    /// The display id, from `lpctl screen displays`; the primary one by default.
    #[arg(long)]
    display: Option<String>,
    /// The widest picture wanted.
    #[arg(long, default_value_t = 1920)]
    width: u32,
    /// The tallest picture wanted.
    #[arg(long, default_value_t = 1080)]
    height: u32,
    /// The frame rate wanted.
    #[arg(long, default_value_t = 30)]
    rate: u32,
    /// Watch only: never drive the device.
    #[arg(long)]
    view_only: bool,
    /// Show the picture at its actual size, not scaled to the window: a point
    /// on the device's screen is a point in the window, whatever the high-DPI
    /// scale at each end. A larger picture scrolls (the wheel while not
    /// driving, the pointer at an edge while driving); `--width` and
    /// `--height` are then ignored.
    #[arg(long)]
    actual_size: bool,
    /// Offer only this chroma sampling: `4:2:0` to spend less of a slow link,
    /// `4:4:4` to insist on full-colour text. By default both are offered,
    /// 4:4:4 first, and the device picks the first it can encode.
    #[arg(long, value_parser = ["4:2:0", "4:4:4"])]
    chroma: Option<String>,
    /// The daemon's data directory (default: the platform's).
    #[arg(long, env = DATA_DIR_ENV)]
    data_dir: Option<PathBuf>,
    /// The daemon's runtime directory, holding the control socket.
    #[arg(long, env = RUNTIME_DIR_ENV)]
    runtime_dir: Option<PathBuf>,
}

/// What the network thread tells the window.
#[derive(Debug)]
enum UserEvent {
    /// The share runs.
    Started(Started),
    /// A new picture is in the shared slot.
    Frame,
    /// Driving began or ended.
    Driving(bool),
    /// The device takes no input from a viewer: why, for the title.
    WatchOnly(String),
    /// The share ended, with why.
    Ended(String),
    /// Shown at its actual size: the shared display's width in pixels and
    /// its scale (ADR 0045), which the picture's zoom is worked out from.
    Actual(u32, f64),
}

/// What the window asks of the network thread.
#[derive(Debug)]
enum Command_ {
    /// Start driving with the pointer at this point of the picture.
    Drive(f64, f64),
    /// Events for the device being driven.
    Events(Vec<InputEvent>),
    /// Stop driving.
    Release,
}

/// The latest decoded picture: width, height and 0RGB pixels.
type Picture = Option<(u32, u32, Vec<u32>)>;

fn main() {
    let options = Options::parse();
    let locations = Locations::resolve(options.data_dir.as_deref(), options.runtime_dir.as_deref());
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("lpview: no display to open a window on: {e}");
            std::process::exit(1);
        }
    };
    let proxy = event_loop.create_proxy();
    let picture: Arc<Mutex<Picture>> = Arc::new(Mutex::new(None));
    let (commands, commands_rx) = mpsc::unbounded_channel::<Command_>();
    let decoder_slot: DecoderSlot = Arc::new(Mutex::new(None));
    // The window's scale, once it exists, for a request at the actual size.
    let (scale_tx, scale_rx) = tokio::sync::oneshot::channel::<f64>();
    {
        let picture = picture.clone();
        let decoder_slot = decoder_slot.clone();
        let options = OptionsForNet::from(&options);
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("a runtime for lpview");
            // Ctrl+C in the terminal, or a SIGTERM, closes the window as its
            // close button does, so `main` stops the decoder before exiting.
            let stop_proxy = proxy.clone();
            runtime.spawn(async move {
                if stop_signal().await {
                    let _ = stop_proxy.send_event(UserEvent::Ended("stopped".to_owned()));
                }
            });
            let ended = runtime.block_on(network(
                options,
                locations,
                proxy.clone(),
                picture,
                commands_rx,
                decoder_slot,
                scale_rx,
            ));
            let message = match ended {
                Ok(reason) => format!("the share ended ({reason})"),
                Err(e) => e.to_string(),
            };
            let _ = proxy.send_event(UserEvent::Ended(message));
        });
    }
    let mut app = App {
        device: options.device.clone(),
        view_only: options.view_only,
        window: None,
        surface: None,
        picture,
        commands,
        started: None,
        driving: false,
        cursor: None,
        target: (0.0, 0.0, 1.0, 1.0),
        held_ctrl: (false, false),
        held_modifiers: Vec::new(),
        modifiers: ModifiersState::empty(),
        unsent: Vec::new(),
        ended: None,
        debug: std::env::var_os("LPVIEW_DEBUG").is_some(),
        watch_only: None,
        scale_tx: Some(scale_tx),
        actual: None,
        scroll: (0.0, 0.0),
        pan: (0.0, 0.0),
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("lpview: {e}");
    }
    // The window is gone: stop the decoder before the process ends, or it
    // writes on into a closed pipe and reports each failure.
    stop_decoder(&decoder_slot);
    if let Some(message) = app.ended {
        eprintln!("lpview: {message}");
    }
}

/// What the network thread needs of the options.
#[derive(Debug, Clone)]
struct OptionsForNet {
    device: String,
    display: Option<String>,
    width: u32,
    height: u32,
    rate: u32,
    actual_size: bool,
    formats: Option<Vec<Format>>,
}

impl From<&Options> for OptionsForNet {
    fn from(o: &Options) -> Self {
        Self {
            device: o.device.clone(),
            display: o.display.clone(),
            width: o.width,
            height: o.height,
            rate: o.rate,
            actual_size: o.actual_size,
            formats: o.chroma.as_deref().map(|c| {
                vec![if c == "4:4:4" {
                    Format::H264_444
                } else {
                    Format::H264
                }]
            }),
        }
    }
}

/// A device by its key, or by its name or a prefix of its key or
/// fingerprint among this person's bindings, as `lpctl` resolves it.
async fn resolve_device(client: &mut Client, device: &str) -> Result<PublicKey, Error> {
    if let Ok(key) = PublicKey::parse(device) {
        return Ok(key);
    }
    let known: Devices = client.request(Request::new(types::DEVICES, 0)).await?;
    let needle = device.to_lowercase();
    let matches: Vec<PublicKey> = known
        .devices
        .iter()
        .filter(|d| d.role == "binding" && !d.this_device && !d.revoked)
        .filter(|d| {
            d.display_name
                .as_deref()
                .is_some_and(|n| n.to_lowercase() == needle)
                || d.key.to_string().starts_with(device)
                || d.key.fingerprint().starts_with(device)
        })
        .map(|d| d.key)
        .collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(Error::Decode(format!(
            "no device named {device}; see lpctl devices"
        ))),
        many => Err(Error::Decode(format!(
            "{device} names {} devices; use the key",
            many.len()
        ))),
    }
}

/// Why a viewer cannot drive a device with these input params, if it cannot:
/// a viewer drives with absolute positions, which the device must accept.
fn watch_only(input: Option<InputParams>) -> Option<&'static str> {
    match input {
        None => Some("it offers no input"),
        Some(p) if !p.accepts => Some("it does not take input"),
        Some(p) if !p.absolute => Some("it does not take absolute positions"),
        Some(_) => None,
    }
}

/// The arguments of the ffmpeg that decodes the share's H.264 into raw 0RGB
/// pictures of its size.
fn decoder_args(started: &Started) -> Vec<String> {
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
    args.extend(
        [
            "-pix_fmt",
            "bgr0",
            // Every decoded picture as it comes. A raw stream carries no
            // timestamps, and VideoToolbox's has no frame timing either, so
            // ffmpeg would otherwise fit the pictures to a rate it guessed and
            // drop nearly all of them.
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "pipe:1",
        ]
        .map(str::to_owned),
    );
    args
}

/// The ffmpeg that decodes the share's H.264 into raw 0RGB pictures of its
/// size, as fast as they come. Not `-fflags nobuffer`: it throws away the
/// packets read while probing, so nothing decodes until the next keyframe.
fn decoder(started: &Started) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt as _;
    Command::new("ffmpeg")
        .args(decoder_args(started))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        // A group of its own, so the terminal's Ctrl+C reaches `lpview`
        // alone; ffmpeg would otherwise finish on its own and report each
        // write into the pipe of an `lpview` already gone.
        .process_group(0)
        .spawn()
}

/// The decoder's pictures, read one at a time, with a picture identical to
/// the one before passed over: a still screen still comes at the agreed
/// rate, and the same picture again needs no drawing.
struct Pictures {
    raw: Vec<u8>,
    last: Vec<u8>,
    have: bool,
}

impl Pictures {
    /// Pictures of `size` bytes each.
    fn new(size: usize) -> Self {
        Self {
            raw: vec![0; size],
            last: vec![0; size],
            have: false,
        }
    }

    /// Reads the next picture: `Some` with its bytes when it differs from the
    /// one before, `None` when it is the same.
    fn next(&mut self, r: &mut impl Read) -> std::io::Result<Option<&[u8]>> {
        r.read_exact(&mut self.raw)?;
        if self.have && self.raw == self.last {
            return Ok(None);
        }
        // Both buffers keep the picture's size, so the next read fills a
        // whole picture whichever it lands in.
        std::mem::swap(&mut self.raw, &mut self.last);
        self.have = true;
        Ok(Some(&self.last))
    }
}

/// How a picture is sized in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Fit {
    /// Scaled to fit, its shape kept.
    Window,
    /// So many window pixels to a picture pixel, scrolled where larger.
    Zoom(f64),
}

/// The picture size to ask a sharer for to show `display` at its actual
/// size in a window of `viewer_scale` pixels per point: the display's points
/// in this window's pixels. The sharer sends no more pixels than it has.
fn actual_request(display: &Display, viewer_scale: f64) -> (u32, u32) {
    let k = viewer_scale / display.scale();
    let size = |px: u32| ((f64::from(px) * k).round() as u32).max(2);
    (size(display.width), size(display.height))
}

/// Window pixels per picture pixel for the actual size: a picture of
/// `picture_width` pixels of a display `display_width` pixels wide at
/// `display_scale`, shown at `viewer_scale`. Close to a whole number, it is
/// that number, so pixels map one to one or to whole squares.
fn actual_zoom(
    display_width: u32,
    display_scale: f64,
    picture_width: u32,
    viewer_scale: f64,
) -> f64 {
    let z = f64::from(display_width) / f64::from(picture_width.max(1)) * viewer_scale
        / display_scale.max(f64::MIN_POSITIVE);
    let whole = z.round();
    if whole >= 1.0 && (z - whole).abs() < 0.02 {
        whole
    } else {
        z
    }
}

/// The scroll offset kept within what a picture of `drawn` window pixels
/// can scroll in a window of `window` pixels: none along a side that fits.
fn clamp_scroll(scroll: (f64, f64), drawn: (f64, f64), (ww, wh): (u32, u32)) -> (f64, f64) {
    let max_x = (drawn.0 - f64::from(ww)).max(0.0);
    let max_y = (drawn.1 - f64::from(wh)).max(0.0);
    (scroll.0.clamp(0.0, max_x), scroll.1.clamp(0.0, max_y))
}

/// Where a picture of `pw` by `ph` lies in a window of `ww` by `wh`: left,
/// top, and window pixels per picture pixel. Along a side where it fits it
/// is centred; along one where it is larger it starts at `-scroll`.
fn place(
    (pw, ph): (u32, u32),
    (ww, wh): (u32, u32),
    fit: Fit,
    scroll: (f64, f64),
) -> (f64, f64, f64) {
    let (fw, fh) = (f64::from(ww), f64::from(wh));
    let zoom = match fit {
        Fit::Window => (fw / f64::from(pw.max(1))).min(fh / f64::from(ph.max(1))),
        Fit::Zoom(z) => z,
    };
    let (dw, dh) = (f64::from(pw) * zoom, f64::from(ph) * zoom);
    let (sx, sy) = clamp_scroll(scroll, (dw, dh), (ww, wh));
    let x = if dw <= fw { (fw - dw) / 2.0 } else { -sx };
    let y = if dh <= fh { (fh - dh) / 2.0 } else { -sy };
    (x, y, zoom)
}

/// Draws a picture of `pw` by `ph` pixels into a window buffer of `ww` by
/// `wh` at `(x, y)` and `zoom` window pixels per picture pixel, the rest
/// black. Returns where it lies: left, top, width and height in window
/// pixels, which may reach past the window.
fn draw(
    pixels: &[u32],
    (pw, ph): (u32, u32),
    buffer: &mut [u32],
    (ww, wh): (u32, u32),
    (x, y, zoom): (f64, f64, f64),
) -> (f64, f64, f64, f64) {
    let (fw, fh) = (f64::from(ww), f64::from(wh));
    if pw == 0 || ph == 0 || zoom <= 0.0 || pixels.len() < pw as usize * ph as usize {
        buffer.fill(0);
        return (0.0, 0.0, fw, fh);
    }
    let (dw, dh) = (f64::from(pw) * zoom, f64::from(ph) * zoom);
    let x0 = x.max(0.0).ceil() as usize;
    let y0 = y.max(0.0).ceil() as usize;
    let x1 = (x + dw).clamp(0.0, fw) as usize;
    let y1 = (y + dh).clamp(0.0, fh) as usize;
    let (row, pw, ph) = (ww as usize, pw as usize, ph as usize);
    // Which source column each window column shows, worked out once.
    let columns: Vec<usize> = (x0..x1.max(x0))
        .map(|wx| (((wx as f64 - x) / zoom) as usize).min(pw - 1))
        .collect();
    for (wy, dst) in buffer.chunks_exact_mut(row).take(wh as usize).enumerate() {
        if wy < y0 || wy >= y1 || x0 >= x1 {
            dst.fill(0);
            continue;
        }
        dst[..x0].fill(0);
        dst[x1..].fill(0);
        let sy = (((wy as f64 - y) / zoom) as usize).min(ph - 1);
        let src = &pixels[sy * pw..(sy + 1) * pw];
        for (out, &sx) in dst[x0..x1].iter_mut().zip(&columns) {
            *out = src[sx];
        }
    }
    (x, y, dw, dh)
}

/// A window position as fractions of a picture drawn at `(x, y, w, h)`,
/// which may reach past the window when it is scrolled; `None` off it.
fn fraction_of((x, y, w, h): (f64, f64, f64, f64), (px, py): (f64, f64)) -> Option<(f64, f64)> {
    let fx = (px - x) / w;
    let fy = (py - y) / h;
    ((0.0..=1.0).contains(&fx) && (0.0..=1.0).contains(&fy)).then_some((fx, fy))
}

/// Draws a picture scaled to fit the window, centred, the margins black.
#[cfg(test)]
fn blit(
    pixels: &[u32],
    picture: (u32, u32),
    buffer: &mut [u32],
    window: (u32, u32),
) -> (f64, f64, f64, f64) {
    draw(
        pixels,
        picture,
        buffer,
        window,
        place(picture, window, Fit::Window, (0.0, 0.0)),
    )
}

/// Waits for Ctrl+C or SIGTERM; false if neither can be watched.
async fn stop_signal() -> bool {
    use tokio::signal::unix::{SignalKind, signal};
    let Ok(mut term) = signal(SignalKind::terminate()) else {
        return tokio::signal::ctrl_c().await.is_ok();
    };
    tokio::select! {
        interrupted = tokio::signal::ctrl_c() => interrupted.is_ok(),
        _ = term.recv() => true,
    }
}

/// Runs the share and the drive until the share ends; returns why it ended.
/// The running decoder, where both the network thread and `main` can stop it.
type DecoderSlot = Arc<Mutex<Option<std::process::Child>>>;

/// Kills and reaps the decoder, if one runs.
fn stop_decoder(slot: &DecoderSlot) {
    let child = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
}

async fn network(
    options: OptionsForNet,
    locations: Locations,
    proxy: EventLoopProxy<UserEvent>,
    picture: Arc<Mutex<Picture>>,
    mut commands: mpsc::UnboundedReceiver<Command_>,
    decoder_slot: DecoderSlot,
    viewer_scale: tokio::sync::oneshot::Receiver<f64>,
) -> Result<String, Error> {
    let socket = locations.control_socket();
    let mut watch = Client::connect(&socket).await?;
    let peer = resolve_device(&mut watch, &options.device).await?;
    let caps: Caps = watch.request(Request::new(types::CAPS, 0)).await?;
    let input = caps
        .peers
        .iter()
        .find(|p| p.peer == peer)
        .and_then(|p| p.capabilities.iter().find_map(InputParams::from_descriptor));
    if let Some(why) = watch_only(input) {
        let _ = proxy.send_event(UserEvent::WatchOnly(why.to_owned()));
    }

    // The drive, on a connection of its own: one batch of events every few
    // milliseconds while driving.
    let mut drive = Client::connect(&socket).await?;
    let driver_proxy = proxy.clone();
    let driver = tokio::spawn(async move {
        let mut batch: Vec<InputEvent> = Vec::new();
        let mut tick = tokio::time::interval(Duration::from_millis(8));
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command_::Drive(x, y)) => {
                        let mut req = Request::new(types::INPUT_DRIVE, 0);
                        req.key = Some(peer);
                        req.x = Some(x);
                        req.y = Some(y);
                        let driven: Result<InputDriven, Error> = drive.request(req).await;
                        if std::env::var_os("LPVIEW_DEBUG").is_some() {
                            eprintln!("lpview: input.drive at {x},{y}: {driven:?}");
                        }
                        if let Err(e) = &driven {
                            eprintln!("lpview: cannot drive: {e}");
                        }
                        let _ = driver_proxy.send_event(UserEvent::Driving(driven.is_ok()));
                    }
                    Some(Command_::Events(events)) => batch.extend(events),
                    Some(Command_::Release) => {
                        batch.clear();
                        let _: Result<Empty, Error> =
                            drive.request(Request::new(types::INPUT_RELEASE, 0)).await;
                    }
                    None => {
                        let _: Result<Empty, Error> =
                            drive.request(Request::new(types::INPUT_RELEASE, 0)).await;
                        return;
                    }
                },
                _ = tick.tick() => {
                    if batch.is_empty() {
                        continue;
                    }
                    let mut req = Request::new(types::INPUT_SEND, 0);
                    req.events = Some(std::mem::take(&mut batch));
                    let sent: Result<Empty, Error> = drive.request(req).await;
                    if sent.is_err() {
                        // The drive ended on the daemon's side: the chord,
                        // the device going away.
                        let _ = driver_proxy.send_event(UserEvent::Driving(false));
                    }
                }
            }
        }
    });

    // At the actual size, the picture asked for is the display's points in
    // this window's pixels (ADR 0045).
    let (width, height) = if options.actual_size {
        let mut req = Request::new(types::SCREEN_DISPLAYS, 0);
        req.key = Some(peer);
        let listed: ScreenDisplays = watch.request(req).await?;
        let display = match &options.display {
            Some(id) => listed.displays.iter().find(|d| &d.id == id),
            None => listed
                .displays
                .iter()
                .find(|d| d.primary)
                .or(listed.displays.first()),
        }
        .ok_or_else(|| Error::Decode("the device has no such display".to_owned()))?;
        let scale = tokio::time::timeout(Duration::from_secs(5), viewer_scale)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(1.0);
        let _ = proxy.send_event(UserEvent::Actual(display.width, display.scale()));
        actual_request(display, scale)
    } else {
        (options.width, options.height)
    };

    // The share: started, then the frames into the decoder.
    let mut req = Request::new(types::SCREEN_START, 0);
    req.key = Some(peer);
    req.display = options.display.clone();
    req.width = Some(width);
    req.height = Some(height);
    req.formats = options.formats.clone();
    req.frame_rate = Some(options.rate);
    let mut feed: Option<std::process::ChildStdin> = None;
    let response = watch
        .call(req, |event: &Event| {
            if let Some(started) = &event.started {
                match decoder(started) {
                    Ok(mut c) => {
                        feed = c.stdin.take();
                        if let Some(mut out) = c.stdout.take() {
                            let (w, h) = (started.width, started.height);
                            let picture = picture.clone();
                            let proxy = proxy.clone();
                            std::thread::spawn(move || {
                                let mut pictures = Pictures::new(w as usize * h as usize * 4);
                                while let Ok(new) = pictures.next(&mut out) {
                                    let Some(new) = new else { continue };
                                    let pixels: Vec<u32> = new
                                        .as_chunks::<4>()
                                        .0
                                        .iter()
                                        .map(|p| u32::from_le_bytes([p[0], p[1], p[2], 0]))
                                        .collect();
                                    *picture
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                        Some((w, h, pixels));
                                    if proxy.send_event(UserEvent::Frame).is_err() {
                                        // The window is gone. Read on until
                                        // `main` stops the decoder: closing its
                                        // output first would have it report a
                                        // broken pipe on every write.
                                        let _ = std::io::copy(&mut out, &mut std::io::sink());
                                        return;
                                    }
                                }
                            });
                        }
                        *decoder_slot
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(c);
                    }
                    Err(e) => eprintln!("lpview: cannot start ffmpeg to decode: {e}"),
                }
                let _ = proxy.send_event(UserEvent::Started(started.clone()));
            }
            if let Some(frame) = &event.frame
                && let Some(stdin) = feed.as_mut()
                && stdin.write_all(&frame.bytes.0).is_err()
            {
                feed = None;
            }
            None
        })
        .await?;
    drop(feed);
    stop_decoder(&decoder_slot);
    driver.abort();
    let ended: ScreenStopped = response.into_ok()?;
    Ok(ended.reason.to_string())
}

struct App {
    device: String,
    view_only: bool,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    picture: Arc<Mutex<Picture>>,
    commands: mpsc::UnboundedSender<Command_>,
    started: Option<Started>,
    driving: bool,
    cursor: Option<PhysicalPosition<f64>>,
    /// Where the picture is drawn in the window: x, y, width, height.
    target: (f64, f64, f64, f64),
    held_ctrl: (bool, bool),
    /// Modifier keys sent as pressed and not yet released, by evdev code.
    held_modifiers: Vec<u16>,
    /// The window's modifier state, for each key's mask.
    modifiers: ModifiersState,
    /// Keys whose press was not sent (a dead key waiting for the next), so
    /// their release is not sent either.
    unsent: Vec<u16>,
    ended: Option<String>,
    /// `LPVIEW_DEBUG` set: every window event is printed.
    debug: bool,
    /// Why the device cannot be driven, when it cannot.
    watch_only: Option<String>,
    /// Where the window's scale goes once the window exists.
    scale_tx: Option<tokio::sync::oneshot::Sender<f64>>,
    /// At the actual size: the shared display's width in pixels and its
    /// scale; scaled to fit when `None`.
    actual: Option<(u32, f64)>,
    /// How far a picture larger than the window is scrolled, in window
    /// pixels.
    scroll: (f64, f64),
    /// Window pixels a frame to scroll while the pointer rests at an edge.
    pan: (f64, f64),
}

/// How close to a window's edge, in points, the pointer pans a larger
/// picture while driving.
const PAN_EDGE: f64 = 24.0;
/// How far it pans each frame, in points.
const PAN_STEP: f64 = 16.0;
/// How often a pan steps.
const PAN_EVERY: Duration = Duration::from_millis(16);
/// Points a wheel step scrolls.
const WHEEL_LINE: f64 = 48.0;

impl App {
    fn title(&self) -> String {
        let state = match (&self.started, self.driving, self.view_only) {
            (None, _, _) => "connecting…".to_owned(),
            (Some(s), true, _) => format!("{} — driving; both Ctrl keys to let go", s.display),
            (Some(s), false, true) => match &self.watch_only {
                Some(why) => format!("{} — watch only: {why}", s.display),
                None => s.display.clone(),
            },
            (Some(s), false, false) => format!("{} — click to drive", s.display),
        };
        match &self.started {
            Some(s) => format!("{} · {state} · {}", self.device, s.format),
            None => format!("{} · {state}", self.device),
        }
    }

    fn retitle(&self) {
        if let Some(w) = &self.window {
            w.set_title(&self.title());
        }
    }

    /// The pointer's position as fractions of the picture, if over it.
    fn fraction(&self, at: PhysicalPosition<f64>) -> Option<(f64, f64)> {
        fraction_of(self.target, (at.x, at.y))
    }

    fn send(&self, events: Vec<InputEvent>) {
        if self.driving {
            let _ = self.commands.send(Command_::Events(events));
        }
    }

    fn release(&mut self) {
        if self.driving {
            let _ = self.commands.send(Command_::Release);
            self.driving = false;
            self.retitle();
        }
    }

    /// Scrolls a larger picture by `(dx, dy)` window pixels; whether it moved.
    fn scroll_by(&mut self, dx: f64, dy: f64) -> bool {
        let (Some(window), Some((_, _, w, h))) = (&self.window, Some(self.target)) else {
            return false;
        };
        let size = window.inner_size();
        let before = self.scroll;
        self.scroll = clamp_scroll(
            (self.scroll.0 + dx, self.scroll.1 + dy),
            (w, h),
            (size.width, size.height),
        );
        if self.scroll != before {
            window.request_redraw();
            true
        } else {
            false
        }
    }

    /// The pan for the pointer at `at` while driving a picture larger than
    /// the window: toward each edge it is within [`PAN_EDGE`] of.
    fn pan_at(&self, at: PhysicalPosition<f64>) -> (f64, f64) {
        let Some(window) = &self.window else {
            return (0.0, 0.0);
        };
        if self.actual.is_none() || !self.driving {
            return (0.0, 0.0);
        }
        let k = window.scale_factor();
        let size = window.inner_size();
        let (ww, wh) = (f64::from(size.width), f64::from(size.height));
        let (_, _, w, h) = self.target;
        let (edge, step) = (PAN_EDGE * k, PAN_STEP * k);
        let along = |pos: f64, extent: f64, drawn: f64| {
            if drawn <= extent {
                0.0
            } else if pos < edge {
                -step
            } else if pos > extent - edge {
                step
            } else {
                0.0
            }
        };
        (along(at.x, ww, w), along(at.y, wh, h))
    }

    fn redraw(&mut self) {
        let actual = self.actual;
        let (Some(window), Some(surface)) = (&self.window, self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        let viewer_scale = window.scale_factor();
        let (Some(ww), Some(wh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if surface.resize(ww, wh).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        let guard = self
            .picture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.as_ref() {
            Some((pw, ph, pixels)) => {
                let (picture, window) = ((*pw, *ph), (size.width, size.height));
                let fit = match actual {
                    Some((dw, ds)) => Fit::Zoom(actual_zoom(dw, ds, *pw, viewer_scale)),
                    None => Fit::Window,
                };
                let placed = place(picture, window, fit, self.scroll);
                if let Fit::Zoom(z) = fit {
                    self.scroll = clamp_scroll(
                        self.scroll,
                        (f64::from(*pw) * z, f64::from(*ph) * z),
                        window,
                    );
                }
                self.target = draw(pixels, picture, &mut buffer, window, placed);
            }
            None => buffer.fill(0),
        }
        drop(guard);
        let _ = buffer.present();
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        use winit::event_loop::ControlFlow;
        if self.pan == (0.0, 0.0) || !self.driving {
            self.pan = (0.0, 0.0);
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let now = std::time::Instant::now();
        if let ControlFlow::WaitUntil(due) = event_loop.control_flow()
            && due > now
        {
            return;
        }
        let (dx, dy) = self.pan;
        if self.scroll_by(dx, dy) {
            // What is under the pointer moved: the driven pointer follows.
            let moved = (
                self.target.0 - dx,
                self.target.1 - dy,
                self.target.2,
                self.target.3,
            );
            self.target = moved;
            if let Some((x, y)) = self.cursor.and_then(|c| self.fraction(c)) {
                self.send(vec![InputEvent::Position { x, y }]);
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(now + PAN_EVERY));
        } else {
            self.pan = (0.0, 0.0);
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.title())
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attributes) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.ended = Some(format!("cannot open a window: {e}"));
                event_loop.exit();
                return;
            }
        };
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|context| softbuffer::Surface::new(&context, window.clone()));
        match surface {
            Ok(s) => self.surface = Some(s),
            Err(e) => {
                self.ended = Some(format!("cannot draw in the window: {e}"));
                event_loop.exit();
                return;
            }
        }
        if let Some(tx) = self.scale_tx.take() {
            let _ = tx.send(window.scale_factor());
        }
        self.window = Some(window);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Started(started) => {
                // At the actual size the window asks to fit the picture; the
                // compositor may tile it otherwise, and the picture scrolls.
                if let (Some((dw, ds)), Some(w)) = (self.actual, &self.window) {
                    let z = actual_zoom(dw, ds, started.width, w.scale_factor());
                    let _ = w.request_inner_size(winit::dpi::PhysicalSize::new(
                        (f64::from(started.width) * z).round() as u32,
                        (f64::from(started.height) * z).round() as u32,
                    ));
                }
                self.started = Some(started);
                self.retitle();
            }
            UserEvent::Frame => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            UserEvent::WatchOnly(why) => {
                self.view_only = true;
                self.watch_only = Some(why);
                self.retitle();
            }
            UserEvent::Driving(driving) => {
                self.driving = driving;
                self.retitle();
            }
            UserEvent::Ended(message) => {
                self.ended = Some(message);
                event_loop.exit();
            }
            UserEvent::Actual(width, scale) => self.actual = Some((width, scale)),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if self.debug && !matches!(event, WindowEvent::RedrawRequested) {
            eprintln!("lpview: {event:?}");
        }
        match event {
            WindowEvent::CloseRequested => {
                self.release();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::Focused(false) => self.release(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some(position);
                if self.driving
                    && let Some((x, y)) = self.fraction(position)
                {
                    self.send(vec![InputEvent::Position { x, y }]);
                }
                let pan = self.pan_at(position);
                if pan != self.pan {
                    self.pan = pan;
                    if pan != (0.0, 0.0) {
                        event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                            std::time::Instant::now() + PAN_EVERY,
                        ));
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                if !self.driving {
                    if self.debug {
                        eprintln!(
                            "lpview: click while not driving: started {} cursor {:?} target {:?} fraction {:?}",
                            self.started.is_some(),
                            self.cursor,
                            self.target,
                            self.cursor.and_then(|c| self.fraction(c))
                        );
                    }
                    if pressed
                        && button == MouseButton::Left
                        && !self.view_only
                        && self.started.is_some()
                        && let Some((x, y)) = self.cursor.and_then(|c| self.fraction(c))
                    {
                        // The first click takes control; it is not sent.
                        let _ = self.commands.send(Command_::Drive(x, y));
                    }
                    return;
                }
                let code = match button {
                    MouseButton::Left => BTN_LEFT,
                    MouseButton::Right => BTN_RIGHT,
                    MouseButton::Middle => BTN_MIDDLE,
                    MouseButton::Back => 275,
                    MouseButton::Forward => 276,
                    MouseButton::Other(_) => return,
                };
                self.send(vec![InputEvent::Button { code, pressed }]);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !self.driving && self.actual.is_some() {
                    // Not driving, the wheel scrolls a picture larger than
                    // the window; Shift turns a vertical wheel sideways.
                    let k = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
                    let (dx, dy) = match delta {
                        MouseScrollDelta::LineDelta(x, y) => (
                            -f64::from(x) * WHEEL_LINE * k,
                            -f64::from(y) * WHEEL_LINE * k,
                        ),
                        MouseScrollDelta::PixelDelta(p) => (-p.x, -p.y),
                    };
                    let (dx, dy) = if self.modifiers.shift_key() && dx == 0.0 {
                        (dy, 0.0)
                    } else {
                        (dx, dy)
                    };
                    self.scroll_by(dx, dy);
                    return;
                }
                let event = match delta {
                    // winit's positive is away from the user; a wheel step
                    // down is positive on the wire, as on Wayland.
                    MouseScrollDelta::LineDelta(x, y) => InputEvent::Scroll {
                        dx: -f64::from(x),
                        dy: -f64::from(y),
                        discrete: true,
                    },
                    MouseScrollDelta::PixelDelta(p) => InputEvent::Scroll {
                        dx: -p.x,
                        dy: -p.y,
                        discrete: false,
                    },
                };
                self.send(vec![event]);
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                // A modifier's release the window did not report as a key
                // would leave it held on the driven device: whatever the
                // window says is up is released there.
                self.modifiers = modifiers.state();
                let stale = stale_modifiers(&self.held_modifiers, modifiers.state());
                if !stale.is_empty() {
                    self.held_modifiers.retain(|c| !stale.contains(c));
                    self.send(
                        stale
                            .into_iter()
                            .map(|code| InputEvent::key(code, false))
                            .collect(),
                    );
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.repeat {
                    return;
                }
                let PhysicalKey::Code(key) = event.physical_key else {
                    return;
                };
                let Some(code) = evdev_code(key) else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;
                match code {
                    KEY_LEFTCTRL => self.held_ctrl.0 = pressed,
                    KEY_RIGHTCTRL => self.held_ctrl.1 = pressed,
                    _ => {}
                }
                let mods = Some(mask_of(self.modifiers));
                if cfg!(target_os = "macos") && code == KEY_CAPSLOCK {
                    // macOS reports Caps Lock as its state changes, pressed
                    // when it turns on and released when it turns off; the
                    // driven device toggles on a press, so each change is a
                    // whole tap.
                    self.send(vec![
                        InputEvent::Key {
                            code,
                            pressed: true,
                            id: Some(keyid::CAPS_LOCK),
                            mods,
                        },
                        InputEvent::key(code, false),
                    ]);
                    return;
                }
                if modifier_flag(code).is_some() {
                    self.held_modifiers.retain(|c| *c != code);
                    if pressed {
                        self.held_modifiers.push(code);
                    }
                }
                if !pressed {
                    // A release is matched to its press by position.
                    if self.unsent.contains(&code) {
                        self.unsent.retain(|c| *c != code);
                        return;
                    }
                    self.send(vec![InputEvent::key(code, false)]);
                } else {
                    let id = match &event.logical_key {
                        // A dead key waits for the key that completes it,
                        // which carries the composed character.
                        Key::Dead(_) => {
                            self.unsent.push(code);
                            return;
                        }
                        Key::Named(named) => named_id(*named, code),
                        Key::Character(_) => {
                            // The character with Shift and Caps Lock, not with
                            // Option (a Mac's Alt), as Synergy computes it.
                            let key = if cfg!(target_os = "macos") && self.modifiers.alt_key() {
                                event.key_without_modifiers()
                            } else {
                                event.logical_key.clone()
                            };
                            let text = event
                                .text
                                .as_ref()
                                .filter(|_| {
                                    !self.modifiers.alt_key() && !self.modifiers.control_key()
                                })
                                .map(ToString::to_string)
                                .or_else(|| match &key {
                                    Key::Character(c) => Some(c.to_string()),
                                    _ => None,
                                });
                            text.and_then(|t| single_char_id(&t, self.modifiers.shift_key()))
                        }
                        _ => None,
                    };
                    self.send(vec![InputEvent::Key {
                        code,
                        pressed,
                        id,
                        mods,
                    }]);
                }
                if self.held_ctrl == (true, true) {
                    self.held_ctrl = (false, false);
                    self.release();
                }
            }
            _ => {}
        }
    }
}

/// Synergy's modifier mask for the window's modifier state.
fn mask_of(state: ModifiersState) -> u32 {
    let mut mask = 0;
    if state.shift_key() {
        mask |= keyid::mask::SHIFT;
    }
    if state.control_key() {
        mask |= keyid::mask::CONTROL;
    }
    if state.alt_key() {
        mask |= keyid::mask::ALT;
    }
    if state.super_key() {
        mask |= keyid::mask::SUPER;
    }
    mask
}

/// The key ID of a single character; a Shift held over a character key
/// whose text came without it (Option stripped) shifts a letter.
fn single_char_id(text: &str, shift: bool) -> Option<KeyId> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() || c.is_control() {
        return None;
    }
    let c = if shift && c.is_lowercase() {
        c.to_uppercase().next().unwrap_or(c)
    } else {
        c
    };
    Some(u32::from(c))
}

/// The key ID of a named key, the side of a modifier from its position.
fn named_id(named: NamedKey, code: u16) -> Option<KeyId> {
    let right = matches!(code, 54 | 97 | 100 | 126);
    let pick = |left: KeyId, right_id: KeyId| Some(if right { right_id } else { left });
    match named {
        NamedKey::Shift => pick(keyid::SHIFT_L, keyid::SHIFT_R),
        NamedKey::Control => pick(keyid::CONTROL_L, keyid::CONTROL_R),
        NamedKey::Alt => pick(keyid::ALT_L, keyid::ALT_R),
        NamedKey::Super => pick(keyid::SUPER_L, keyid::SUPER_R),
        NamedKey::Meta => pick(keyid::META_L, keyid::META_R),
        NamedKey::AltGraph => Some(keyid::ALT_GR),
        NamedKey::CapsLock => Some(keyid::CAPS_LOCK),
        NamedKey::Space => Some(u32::from(' ')),
        _ => keyid::named_for_evdev(code),
    }
}

/// Caps Lock's evdev code.
const KEY_CAPSLOCK: u16 = 58;

/// The modifier flag an evdev modifier key sets, `None` for other keys.
fn modifier_flag(code: u16) -> Option<ModifiersState> {
    match code {
        42 | 54 => Some(ModifiersState::SHIFT),
        KEY_LEFTCTRL | KEY_RIGHTCTRL => Some(ModifiersState::CONTROL),
        56 | 100 => Some(ModifiersState::ALT),
        125 | 126 => Some(ModifiersState::SUPER),
        _ => None,
    }
}

/// The held modifier keys whose flag `state` says is no longer down.
fn stale_modifiers(held: &[u16], state: ModifiersState) -> Vec<u16> {
    held.iter()
        .copied()
        .filter(|code| modifier_flag(*code).is_some_and(|flag| !state.contains(flag)))
        .collect()
}

/// The Linux evdev code of a physical key, from winit's layout-independent
/// code, so the driven device applies its own layout. Keys without an evdev
/// code are not sent.
#[must_use]
fn evdev_code(key: KeyCode) -> Option<u16> {
    use KeyCode as K;
    Some(match key {
        K::Escape => 1,
        K::Digit1 => 2,
        K::Digit2 => 3,
        K::Digit3 => 4,
        K::Digit4 => 5,
        K::Digit5 => 6,
        K::Digit6 => 7,
        K::Digit7 => 8,
        K::Digit8 => 9,
        K::Digit9 => 10,
        K::Digit0 => 11,
        K::Minus => 12,
        K::Equal => 13,
        K::Backspace => 14,
        K::Tab => 15,
        K::KeyQ => 16,
        K::KeyW => 17,
        K::KeyE => 18,
        K::KeyR => 19,
        K::KeyT => 20,
        K::KeyY => 21,
        K::KeyU => 22,
        K::KeyI => 23,
        K::KeyO => 24,
        K::KeyP => 25,
        K::BracketLeft => 26,
        K::BracketRight => 27,
        K::Enter => 28,
        K::ControlLeft => KEY_LEFTCTRL,
        K::KeyA => 30,
        K::KeyS => 31,
        K::KeyD => 32,
        K::KeyF => 33,
        K::KeyG => 34,
        K::KeyH => 35,
        K::KeyJ => 36,
        K::KeyK => 37,
        K::KeyL => 38,
        K::Semicolon => 39,
        K::Quote => 40,
        K::Backquote => 41,
        K::ShiftLeft => 42,
        K::Backslash => 43,
        K::KeyZ => 44,
        K::KeyX => 45,
        K::KeyC => 46,
        K::KeyV => 47,
        K::KeyB => 48,
        K::KeyN => 49,
        K::KeyM => 50,
        K::Comma => 51,
        K::Period => 52,
        K::Slash => 53,
        K::ShiftRight => 54,
        K::NumpadMultiply => 55,
        K::AltLeft => 56,
        K::Space => 57,
        K::CapsLock => 58,
        K::F1 => 59,
        K::F2 => 60,
        K::F3 => 61,
        K::F4 => 62,
        K::F5 => 63,
        K::F6 => 64,
        K::F7 => 65,
        K::F8 => 66,
        K::F9 => 67,
        K::F10 => 68,
        K::NumLock => 69,
        K::ScrollLock => 70,
        K::Numpad7 => 71,
        K::Numpad8 => 72,
        K::Numpad9 => 73,
        K::NumpadSubtract => 74,
        K::Numpad4 => 75,
        K::Numpad5 => 76,
        K::Numpad6 => 77,
        K::NumpadAdd => 78,
        K::Numpad1 => 79,
        K::Numpad2 => 80,
        K::Numpad3 => 81,
        K::Numpad0 => 82,
        K::NumpadDecimal => 83,
        K::IntlBackslash => 86,
        K::F11 => 87,
        K::F12 => 88,
        K::NumpadEnter => 96,
        K::ControlRight => KEY_RIGHTCTRL,
        K::NumpadDivide => 98,
        K::PrintScreen => 99,
        K::AltRight => 100,
        K::Home => 102,
        K::ArrowUp => 103,
        K::PageUp => 104,
        K::ArrowLeft => 105,
        K::ArrowRight => 106,
        K::End => 107,
        K::ArrowDown => 108,
        K::PageDown => 109,
        K::Insert => 110,
        K::Delete => 111,
        K::AudioVolumeMute => 113,
        K::AudioVolumeDown => 114,
        K::AudioVolumeUp => 115,
        K::Pause => 119,
        K::SuperLeft => 125,
        K::SuperRight => 126,
        K::ContextMenu => 127,
        K::MediaTrackNext => 163,
        K::MediaPlayPause => 164,
        K::MediaTrackPrevious => 165,
        K::MediaStop => 166,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_the_same_as_the_last_is_passed_over() {
        // Three pictures of two bytes: A, A again, B, then the end.
        let mut stream: &[u8] = &[1, 2, 1, 2, 3, 4];
        let mut pictures = Pictures::new(2);
        let first = pictures.next(&mut stream).unwrap().map(<[u8]>::to_vec);
        assert_eq!(first, Some(vec![1, 2]));
        assert_eq!(pictures.next(&mut stream).unwrap(), None, "the same again");
        let third = pictures.next(&mut stream).unwrap().map(<[u8]>::to_vec);
        assert_eq!(third, Some(vec![3, 4]), "a whole picture after a swap");
        assert!(pictures.next(&mut stream).is_err(), "the end of the stream");
        // A first picture of all zeros is still drawn.
        let mut zeros: &[u8] = &[0, 0];
        assert!(Pictures::new(2).next(&mut zeros).unwrap().is_some());
    }

    #[test]
    fn a_picture_is_scaled_to_fit_centred_with_black_margins() {
        // Two pixels, A and B, into a 4 by 4 window: twice the size, centred
        // vertically, black above and below.
        let (a, b) = (0x00_aa_aa_aa, 0x00_bb_bb_bb);
        let mut buffer = vec![7u32; 16];
        let target = blit(&[a, b], (2, 1), &mut buffer, (4, 4));
        assert_eq!(target, (0.0, 1.0, 4.0, 2.0));
        assert_eq!(
            buffer,
            [0, 0, 0, 0, a, a, b, b, a, a, b, b, 0, 0, 0, 0],
            "rows of the picture, black margins, nothing left over"
        );
        // Narrower than the window: black at the sides.
        let mut buffer = vec![7u32; 8];
        blit(&[a], (1, 1), &mut buffer, (4, 2));
        assert_eq!(buffer, [0, a, a, 0, 0, a, a, 0]);
    }

    fn display(width: u32, height: u32, scale: Option<f64>) -> Display {
        Display {
            id: "1".to_owned(),
            name: None,
            width,
            height,
            primary: true,
            scale,
        }
    }

    /// The picture asked for is the display's points in the viewer's pixels.
    #[test]
    fn the_actual_size_asks_for_the_display_s_points_in_the_viewer_s_pixels() {
        let retina = display(2880, 1800, Some(2.0));
        assert_eq!(
            actual_request(&retina, 1.0),
            (1440, 900),
            "a Retina Mac on a 1x monitor"
        );
        assert_eq!(
            actual_request(&retina, 2.0),
            (2880, 1800),
            "Retina to Retina"
        );
        let plain = display(1920, 1080, None);
        assert_eq!(actual_request(&plain, 1.0), (1920, 1080), "no scale is 1");
        assert_eq!(
            actual_request(&plain, 2.0),
            (3840, 2160),
            "more than it has: it caps"
        );
        let fractional = display(3840, 2160, Some(1.5));
        assert_eq!(actual_request(&fractional, 1.0), (2560, 1440));
    }

    /// The zoom is one where the sharer scaled down to the request, a whole
    /// number where it could not send enough, and snaps to one near it.
    #[test]
    fn the_actual_size_zoom_maps_pixels_one_to_one_or_to_whole_squares() {
        assert_eq!(actual_zoom(2880, 2.0, 1440, 1.0), 1.0);
        assert_eq!(actual_zoom(2880, 2.0, 2880, 2.0), 1.0);
        assert_eq!(
            actual_zoom(1920, 1.0, 1920, 2.0),
            2.0,
            "a 1x screen on a Retina viewer"
        );
        assert_eq!(
            actual_zoom(2880, 2.0, 1438, 1.0),
            1.0,
            "even rounding by the sharer"
        );
        let z = actual_zoom(3840, 1.5, 3840, 1.0);
        assert!((z - 2.0 / 3.0).abs() < 1e-9, "{z}");
    }

    #[test]
    fn a_picture_smaller_than_the_window_is_centred_and_a_larger_one_scrolls() {
        // 4 by 2 at zoom 1 in a 6 by 4 window: centred.
        assert_eq!(
            place((4, 2), (6, 4), Fit::Zoom(1.0), (0.0, 0.0)),
            (1.0, 1.0, 1.0)
        );
        // 10 by 2 in the same window: scrolled across, centred down.
        assert_eq!(
            place((10, 2), (6, 4), Fit::Zoom(1.0), (3.0, 9.0)),
            (-3.0, 1.0, 1.0)
        );
        // Scrolled past its end: held at the end.
        assert_eq!(
            place((10, 2), (6, 4), Fit::Zoom(1.0), (50.0, 0.0)),
            (-4.0, 1.0, 1.0)
        );
        assert_eq!(clamp_scroll((-5.0, 3.0), (10.0, 2.0), (6, 4)), (0.0, 0.0));
        assert_eq!(clamp_scroll((2.0, 3.0), (10.0, 8.0), (6, 4)), (2.0, 3.0));
        // Fit is unchanged: twice the size of a 2 by 1 picture in 4 by 4.
        assert_eq!(
            place((2, 1), (4, 4), Fit::Window, (9.0, 9.0)),
            (0.0, 1.0, 2.0)
        );
    }

    #[test]
    fn a_scrolled_picture_is_drawn_from_where_it_is_scrolled_to_and_clicks_map_through_it() {
        // Four pixels in a row, a window two wide, scrolled one along.
        let px = [1, 2, 3, 4];
        let mut buffer = vec![7u32; 2];
        let placed = place((4, 1), (2, 1), Fit::Zoom(1.0), (1.0, 0.0));
        let target = draw(&px, (4, 1), &mut buffer, (2, 1), placed);
        assert_eq!(buffer, [2, 3]);
        assert_eq!(target, (-1.0, 0.0, 4.0, 1.0));
        // The window's first pixel is the picture's second: a quarter along.
        assert_eq!(fraction_of(target, (0.0, 0.5)), Some((0.25, 0.5)));
        assert_eq!(fraction_of(target, (5.0, 0.5)), None);
        // At zoom 2, each picture pixel is a square of four.
        let mut buffer = vec![7u32; 8];
        draw(&[1, 2], (2, 1), &mut buffer, (4, 2), (0.0, 0.0, 2.0));
        assert_eq!(buffer, [1, 1, 2, 2, 1, 1, 2, 2]);
    }

    /// Encodes `frames` pictures of a test pattern at 30 a second, a
    /// keyframe each second, as an Annex B stream in `pix_fmt`; `None`
    /// without ffmpeg's `libx264`.
    fn encoded(frames: u32, pix_fmt: &str) -> Option<Vec<u8>> {
        let out = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=30",
                "-frames:v",
                &frames.to_string(),
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                pix_fmt,
                "-g",
                "30",
                "-bf",
                "0",
                "-f",
                "h264",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
    }

    #[test]
    fn keys_carry_their_meaning() {
        assert_eq!(single_char_id("@", true), Some(u32::from('@')));
        assert_eq!(
            single_char_id("e", true),
            Some(u32::from('E')),
            "Option stripped, Shift kept"
        );
        assert_eq!(single_char_id("ab", false), None);
        assert_eq!(single_char_id("\r", false), None);
        assert_eq!(named_id(NamedKey::Alt, 56), Some(keyid::ALT_L));
        assert_eq!(named_id(NamedKey::Super, 126), Some(keyid::SUPER_R));
        assert_eq!(named_id(NamedKey::Enter, 28), Some(keyid::RETURN));
        assert_eq!(named_id(NamedKey::Space, 57), Some(0x20));
        let state = ModifiersState::SHIFT | ModifiersState::CONTROL;
        assert_eq!(mask_of(state), keyid::mask::SHIFT | keyid::mask::CONTROL);
    }

    #[test]
    fn a_modifier_the_window_says_is_up_is_released() {
        let held = [42, 29, 125];
        // Shift's release was lost; Control and Command are still down.
        let state = ModifiersState::CONTROL | ModifiersState::SUPER;
        assert_eq!(stale_modifiers(&held, state), vec![42]);
        assert_eq!(
            stale_modifiers(&held, ModifiersState::empty()),
            vec![42, 29, 125]
        );
        assert!(stale_modifiers(&[], ModifiersState::empty()).is_empty());
        assert_eq!(modifier_flag(30), None, "A is not a modifier");
    }

    #[test]
    fn a_device_that_takes_no_absolute_input_is_only_watched() {
        let params = |accepts, absolute| InputParams {
            accepts,
            provides: true,
            absolute,
            text: accepts,
        };
        assert_eq!(watch_only(None), Some("it offers no input"));
        // The phone: it provides a touchpad and accepts nothing.
        assert_eq!(
            watch_only(Some(params(false, false))),
            Some("it does not take input")
        );
        assert_eq!(
            watch_only(Some(params(true, false))),
            Some("it does not take absolute positions")
        );
        assert_eq!(watch_only(Some(params(true, true))), None);
    }

    #[test]
    fn the_decoder_turns_every_frame_into_a_picture() {
        use lp_core::screen::Format;
        // Both formats a desktop decodes (ADR 0044).
        for (format, pix_fmt) in [(Format::H264, "yuv420p"), (Format::H264_444, "yuv444p")] {
            let Some(stream) = encoded(90, pix_fmt) else {
                eprintln!("skipped: no ffmpeg with libx264 here");
                return;
            };
            let started = Started {
                display: "1".to_owned(),
                format,
                width: 320,
                height: 240,
                frame_rate: 30,
            };
            let mut child = Command::new("ffmpeg")
                .args(decoder_args(&started))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            let writer = std::thread::spawn(move || {
                let _ = stdin.write_all(&stream);
            });
            let mut out = Vec::new();
            child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
            writer.join().unwrap();
            child.wait().unwrap();
            assert_eq!(
                out.len() / (320 * 240 * 4),
                90,
                "a picture for every frame, from the first, at {format}"
            );
        }
    }

    #[test]
    fn physical_keys_map_to_evdev_codes() {
        assert_eq!(evdev_code(KeyCode::KeyA), Some(30));
        assert_eq!(evdev_code(KeyCode::KeyZ), Some(44));
        assert_eq!(evdev_code(KeyCode::Enter), Some(28));
        assert_eq!(evdev_code(KeyCode::ControlLeft), Some(KEY_LEFTCTRL));
        assert_eq!(evdev_code(KeyCode::ControlRight), Some(KEY_RIGHTCTRL));
        assert_eq!(evdev_code(KeyCode::ArrowLeft), Some(105));
        assert_eq!(evdev_code(KeyCode::F12), Some(88));
        assert_eq!(evdev_code(KeyCode::Fn), None);
    }
}
