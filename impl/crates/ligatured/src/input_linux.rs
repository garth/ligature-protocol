//! The input capability on Linux under a Wayland compositor (milestone 7,
//! LP-143): capture through the InputCapture portal and libei, injection
//! through the virtual pointer and virtual keyboard protocols.
//!
//! **Capture** ([`LinuxCapture`]) runs in the daemon. The portal
//! (`org.freedesktop.portal.InputCapture`, which Hyprland's backend
//! implements) owns the pointer barriers: the daemon asks for the zones (the
//! outputs' logical rectangles), sets a barrier along each edge the layout
//! maps, and enables the session. When the pointer crosses a barrier the
//! compositor sends `Activated` with the cursor position, and from then on
//! delivers the keyboard and mouse over a libei socket instead of to the
//! desktop; the daemon turns that into [`CaptureEvent`]s. `Release` with a
//! position gives the input back and places the cursor. Nothing about this
//! needs the compositor's socket, so it runs without a helper.
//!
//! **Injection** ([`LinuxInject`]) runs in a helper child process
//! (`ligatured input-helper`), as the clipboard does and for the same reason:
//! the daemon may start without `WAYLAND_DISPLAY`. The helper binds a
//! virtual pointer and a virtual keyboard with the seat's own keymap, so
//! evdev codes land as the layout says; places the pointer with an absolute
//! motion; and types a `text` event by swapping in a keymap that maps each
//! character to a key, as `wtype` does, then swapping the seat's back. It
//! reports the layout's logical size, from `xdg-output`, as the screen.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lp_core::Error;
use lp_core::frame;
use lp_core::input::{Edge, InputEvent, InputParams};
use lp_core::wire::TypeName;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::clipboard_linux::select_wayland_socket;
use crate::input::geometry::{Zone, back_position, edge_fraction, size};
use crate::input::{Capture, CaptureEvent, Inject};

/// The largest frame between the daemon and its input helper.
const HELPER_MAX_FRAME: usize = 64 * 1024;
/// How often a missing helper is retried.
const RETRY: Duration = Duration::from_secs(3);
/// How often the InputCapture portal is tried again while it is missing.
const PORTAL_RETRY: Duration = Duration::from_secs(30);
/// libei scroll in units of a wheel detent.
const EI_DETENT: f64 = 120.0;
/// Units per wheel step on a `wl_pointer` axis, as libinput reports them.
const WHEEL_STEP: f64 = 15.0;

/// Type names of the helper's messages.
pub mod types {
    /// Helper to daemon: up, with the layout's logical size.
    pub const READY: &str = "org.ligatureprotocol.input-helper.ready";
    /// Daemon to helper: a drive begins at this position (fractions).
    pub const BEGIN: &str = "org.ligatureprotocol.input-helper.begin";
    /// Daemon to helper: inject this event.
    pub const EVENT: &str = "org.ligatureprotocol.input-helper.event";
    /// Daemon to helper: the drive ends; lift everything.
    pub const END: &str = "org.ligatureprotocol.input-helper.end";
}

/// A message between the daemon and its input helper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelperMessage {
    /// One of [`types`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// `ready`: the layout's logical width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// `ready`: the layout's logical height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// `begin`: where, as fractions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    /// `begin`: where, as fractions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// `event`: the event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<InputEvent>,
}

impl HelperMessage {
    fn new(t: &str) -> Self {
        Self {
            message_type: TypeName::new(t),
            width: None,
            height: None,
            x: None,
            y: None,
            event: None,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_frame<W: std::io::Write>(w: &mut W, msg: &HelperMessage) -> Result<(), Error> {
    let bytes = frame::encode(msg, HELPER_MAX_FRAME)?;
    w.write_all(&bytes)
        .and_then(|()| w.flush())
        .map_err(|e| Error::Io(format!("input helper write: {e}")))
}

fn read_frame<R: std::io::Read>(r: &mut R) -> Result<Option<HelperMessage>, Error> {
    let mut prefix = [0u8; 4];
    match r.read_exact(&mut prefix) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(Error::Io(format!("input helper read: {e}"))),
    }
    let len = frame::body_len(prefix, HELPER_MAX_FRAME)?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)
        .map_err(|e| Error::Io(format!("input helper read: {e}")))?;
    frame::decode(&body).map(Some)
}

// ---------------------------------------------------------------------------
// Injection: the daemon's side of the helper.

struct Helper {
    child: Child,
    stdin: ChildStdin,
}

/// The Linux injector: a supervised helper that drives a virtual pointer and
/// keyboard. Implements [`Inject`]; `accepts` while the helper runs.
pub struct LinuxInject {
    helper: Mutex<Option<Helper>>,
    /// The layout's logical size, as the helper last reported it.
    screen: Arc<Mutex<(u32, u32)>>,
    runtime_dir: PathBuf,
    env_display: Option<std::ffi::OsString>,
    /// The executable whose `input-helper` subcommand is the helper: this
    /// binary, or the daemon's when a test process is the parent.
    helper_exe: PathBuf,
}

impl LinuxInject {
    /// Starts the supervisor, which starts a helper whenever a compositor
    /// socket exists and no helper runs. Called within the daemon's runtime.
    #[must_use]
    pub fn start() -> Arc<Self> {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Self::start_with(runtime_dir, std::env::var_os("WAYLAND_DISPLAY"), None)
    }

    /// As [`LinuxInject::start`], with the socket selection's inputs given
    /// and, for a test whose own executable is not the daemon, the daemon
    /// binary that provides the helper.
    #[must_use]
    pub fn start_with(
        runtime_dir: PathBuf,
        env_display: Option<std::ffi::OsString>,
        helper_exe: Option<PathBuf>,
    ) -> Arc<Self> {
        let helper_exe = helper_exe
            .or_else(|| std::env::current_exe().ok())
            .unwrap_or_else(|| PathBuf::from("ligatured"));
        let this = Arc::new(Self {
            helper: Mutex::new(None),
            screen: Arc::new(Mutex::new((0, 0))),
            runtime_dir,
            env_display,
            helper_exe,
        });
        let supervisor = this.clone();
        std::thread::Builder::new()
            .name("input-helper-supervisor".into())
            .spawn(move || supervisor.supervise())
            .expect("spawn the input helper supervisor");
        this
    }

    fn supervise(&self) {
        loop {
            let running = lock(&self.helper)
                .as_mut()
                .is_some_and(|h| matches!(h.child.try_wait(), Ok(None)));
            if !running {
                *lock(&self.helper) = None;
                match select_wayland_socket(self.env_display.as_deref(), &self.runtime_dir) {
                    Some(socket) if socket.exists() => match self.spawn_helper(&socket) {
                        Ok(()) => {
                            tracing::info!(socket = %socket.display(), "input helper started");
                        }
                        Err(e) => tracing::warn!(%e, "input helper did not start; retrying"),
                    },
                    _ => tracing::debug!("no compositor socket for the input helper yet"),
                }
            }
            std::thread::sleep(RETRY);
        }
    }

    fn spawn_helper(&self, socket: &Path) -> Result<(), Error> {
        let mut child = Command::new(&self.helper_exe)
            .arg("input-helper")
            .env("WAYLAND_DISPLAY", socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| Error::Io(format!("spawn input helper: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Io("helper stdin".to_owned()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Io("helper stdout".to_owned()))?;
        let first = read_frame(&mut stdout)?
            .ok_or_else(|| Error::Io("input helper exited at once".to_owned()))?;
        if first.message_type.as_str() != types::READY {
            return Err(Error::Io(format!(
                "helper said {} not ready",
                first.message_type
            )));
        }
        *lock(&self.screen) = (first.width.unwrap_or(0), first.height.unwrap_or(0));
        *lock(&self.helper) = Some(Helper { child, stdin });
        let shared_screen = self.screen.clone();
        std::thread::spawn(move || {
            while let Ok(Some(msg)) = read_frame(&mut stdout) {
                if msg.message_type.as_str() == types::READY {
                    *lock(&shared_screen) = (msg.width.unwrap_or(0), msg.height.unwrap_or(0));
                }
            }
            tracing::info!("input helper ended; injection unavailable until it restarts");
        });
        Ok(())
    }

    fn send(&self, msg: &HelperMessage) -> Result<(), Error> {
        let mut guard = lock(&self.helper);
        let helper = guard
            .as_mut()
            .ok_or_else(|| Error::Io("no input helper is running".to_owned()))?;
        write_frame(&mut helper.stdin, msg)
    }
}

impl Inject for LinuxInject {
    fn params(&self) -> InputParams {
        let accepts = lock(&self.helper).is_some();
        InputParams {
            accepts,
            provides: false,
            absolute: accepts,
            text: accepts,
        }
    }

    fn screen(&self) -> (u32, u32) {
        *lock(&self.screen)
    }

    fn begin(&self, start: (f64, f64)) -> Result<(), Error> {
        let mut msg = HelperMessage::new(types::BEGIN);
        msg.x = Some(start.0);
        msg.y = Some(start.1);
        self.send(&msg)
    }

    fn inject(&self, event: &InputEvent) -> Result<(), Error> {
        let mut msg = HelperMessage::new(types::EVENT);
        msg.event = Some(event.clone());
        self.send(&msg)
    }

    fn end(&self) {
        if let Err(e) = self.send(&HelperMessage::new(types::END)) {
            tracing::debug!(%e, "cannot tell the input helper the drive ended");
        }
    }
}

// ---------------------------------------------------------------------------
// Injection: the helper process.

/// The keymap text that types `text`: one key per distinct character, the
/// characters named as Unicode keysyms, which `xkbcommon` reads without any
/// system data. Returns the keymap and the evdev code of each character.
#[must_use]
pub fn text_keymap(text: &str) -> (String, HashMap<char, u32>) {
    let mut codes: HashMap<char, u32> = HashMap::new();
    let mut keycodes = String::new();
    let mut symbols = String::new();
    for c in text.chars() {
        if codes.contains_key(&c) || codes.len() >= 240 {
            continue;
        }
        // Evdev codes from 1; the xkb keycode is the evdev code plus 8.
        let code = codes.len() as u32 + 1;
        codes.insert(c, code);
        keycodes.push_str(&format!("<K{code}> = {};\n", code + 8));
        symbols.push_str(&format!("key <K{code}> {{ [ U{:04X} ] }};\n", c as u32));
    }
    let keymap = format!(
        "xkb_keymap {{\n\
         xkb_keycodes \"(unnamed)\" {{\nminimum = 8;\nmaximum = 255;\n{keycodes}}};\n\
         xkb_types \"(unnamed)\" {{\ntype \"ONE_LEVEL\" {{ modifiers = none; level_name[Level1] = \"Any\"; }};\n}};\n\
         xkb_compatibility \"(unnamed)\" {{ }};\n\
         xkb_symbols \"(unnamed)\" {{\n{symbols}}};\n\
         }};\n"
    );
    (keymap, codes)
}

/// A keymap for a seat that reported none: the US layout by include, which
/// the compositor's `xkbcommon` resolves from the system's data.
const US_KEYMAP: &str = "xkb_keymap {\n\
    xkb_keycodes { include \"evdev\" };\n\
    xkb_types { include \"complete\" };\n\
    xkb_compatibility { include \"complete\" };\n\
    xkb_symbols { include \"pc+us+inet(evdev)\" };\n\
    };\n";

/// Runs the helper: binds the virtual devices, reports the screen, and
/// injects what stdin says until it closes.
///
/// # Errors
///
/// Returns [`Error::Io`] when the compositor lacks a protocol or a seat.
pub fn run_helper() -> Result<(), Error> {
    helper::run()
}

mod helper {
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::{Read as _, Seek as _, Write as _};
    use std::os::fd::{AsFd as _, OwnedFd};
    use std::time::Instant;

    use lp_core::Error;
    use lp_core::input::InputEvent;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
    use wayland_client::protocol::wl_output::{self, WlOutput};
    use wayland_client::protocol::wl_pointer::{Axis, AxisSource, ButtonState};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::protocol::wl_seat::{self, WlSeat};
    use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle, WEnum};
    use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
    use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};
    use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
    use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
    use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
    use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

    use super::{
        HelperMessage, US_KEYMAP, WHEEL_STEP, read_frame, text_keymap, types, write_frame,
    };

    /// The extent absolute motion is expressed against: fractions times this.
    const EXTENT: u32 = 1_000_000;
    /// The keymap format of `wl_keyboard`: XKB_V1.
    const XKB_V1: u32 = 1;

    #[derive(Default, Clone)]
    struct Output {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        done: bool,
    }

    struct State {
        started: Instant,
        pointer: ZwlrVirtualPointerV1,
        keyboard_manager: ZwpVirtualKeyboardManagerV1,
        seat: WlSeat,
        keyboard: Option<ZwpVirtualKeyboardV1>,
        /// The seat's keymap, as `wl_keyboard` delivered it, or the US one.
        seat_keymap: Option<Vec<u8>>,
        outputs: HashMap<u32, Output>,
        xdg_outputs: HashMap<u32, ZxdgOutputV1>,
        /// Keys and buttons pressed and not yet released.
        held: Vec<InputEvent>,
        reported: Option<(u32, u32)>,
        outputs_changed: bool,
    }

    impl State {
        fn time(&self) -> u32 {
            u32::try_from(self.started.elapsed().as_millis()).unwrap_or(u32::MAX)
        }

        /// The layout's logical bounding box.
        fn screen(&self) -> (u32, u32) {
            let (mut right, mut bottom) = (0i32, 0i32);
            for o in self.outputs.values().filter(|o| o.done) {
                right = right.max(o.x + o.width);
                bottom = bottom.max(o.y + o.height);
            }
            (right.max(0) as u32, bottom.max(0) as u32)
        }

        fn ensure_keyboard(&mut self, qh: &QueueHandle<Self>) -> &ZwpVirtualKeyboardV1 {
            if self.keyboard.is_none() {
                let kb = self
                    .keyboard_manager
                    .create_virtual_keyboard(&self.seat, qh, ());
                let keymap = self
                    .seat_keymap
                    .clone()
                    .unwrap_or_else(|| US_KEYMAP.as_bytes().to_vec());
                set_keymap(&kb, &keymap);
                self.keyboard = Some(kb);
            }
            self.keyboard.as_ref().expect("just created")
        }

        fn begin(&mut self, x: f64, y: f64) {
            let (w, h) = self.screen();
            let _ = (w, h);
            let t = self.time();
            let to = |f: f64| (f.clamp(0.0, 1.0) * f64::from(EXTENT)) as u32;
            self.pointer
                .motion_absolute(t, to(x), to(y), EXTENT, EXTENT);
            self.pointer.frame();
        }

        fn inject(&mut self, qh: &QueueHandle<Self>, event: &InputEvent) {
            let t = self.time();
            match event {
                InputEvent::Motion { dx, dy } => {
                    self.pointer.motion(t, *dx, *dy);
                    self.pointer.frame();
                }
                InputEvent::Position { x, y } => self.begin(*x, *y),
                InputEvent::Button { code, pressed } => {
                    let state = if *pressed {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    };
                    self.pointer.button(t, u32::from(*code), state);
                    self.pointer.frame();
                    self.note_held(event);
                }
                InputEvent::Scroll { dx, dy, discrete } => {
                    if *discrete {
                        self.pointer.axis_source(AxisSource::Wheel);
                        if *dy != 0.0 {
                            self.pointer.axis_discrete(
                                t,
                                Axis::VerticalScroll,
                                dy * WHEEL_STEP,
                                dy.round() as i32,
                            );
                        }
                        if *dx != 0.0 {
                            self.pointer.axis_discrete(
                                t,
                                Axis::HorizontalScroll,
                                dx * WHEEL_STEP,
                                dx.round() as i32,
                            );
                        }
                    } else {
                        self.pointer.axis_source(AxisSource::Finger);
                        if *dy != 0.0 {
                            self.pointer.axis(t, Axis::VerticalScroll, *dy);
                        }
                        if *dx != 0.0 {
                            self.pointer.axis(t, Axis::HorizontalScroll, *dx);
                        }
                    }
                    self.pointer.frame();
                }
                InputEvent::Key { code, pressed } => {
                    let kb = self.ensure_keyboard(qh).clone();
                    kb.key(t, u32::from(*code), u32::from(*pressed));
                    self.note_held(event);
                }
                InputEvent::Text { text } => {
                    // A keyboard of its own for the text, as `wtype` does: the
                    // compositor reads a virtual keyboard's keymap once, when
                    // it is set, so the seat-keymap keyboard stays as it is
                    // and this one lives for the string.
                    let kb = self
                        .keyboard_manager
                        .create_virtual_keyboard(&self.seat, qh, ());
                    let (keymap, codes) = text_keymap(text);
                    set_keymap(&kb, keymap.as_bytes());
                    for c in text.chars() {
                        let Some(code) = codes.get(&c) else { continue };
                        kb.key(t, *code, 1);
                        kb.key(t, *code, 0);
                    }
                    kb.destroy();
                }
            }
        }

        fn note_held(&mut self, event: &InputEvent) {
            let (lifted, pressed) = match event {
                InputEvent::Key { code, pressed } => (
                    InputEvent::Key {
                        code: *code,
                        pressed: false,
                    },
                    *pressed,
                ),
                InputEvent::Button { code, pressed } => (
                    InputEvent::Button {
                        code: *code,
                        pressed: false,
                    },
                    *pressed,
                ),
                _ => return,
            };
            if pressed {
                if !self.held.contains(&lifted) {
                    self.held.push(lifted);
                }
            } else {
                self.held.retain(|h| h != &lifted);
            }
        }

        fn end(&mut self, qh: &QueueHandle<Self>) {
            let held = std::mem::take(&mut self.held);
            for event in &held {
                self.inject(qh, event);
            }
        }
    }

    /// Hands the keyboard a keymap through a memory file.
    fn set_keymap(kb: &ZwpVirtualKeyboardV1, keymap: &[u8]) {
        let Ok(fd) = rustix::fs::memfd_create("ligature-keymap", rustix::fs::MemfdFlags::CLOEXEC)
        else {
            return;
        };
        let mut file = File::from(fd);
        // A keymap is NUL-terminated as the compositor maps it.
        let mut bytes = keymap.to_vec();
        if bytes.last() != Some(&0) {
            bytes.push(0);
        }
        if file.write_all(&bytes).is_err() || file.rewind().is_err() {
            return;
        }
        kb.keymap(XKB_V1, file.as_fd(), bytes.len() as u32);
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

    impl Dispatch<WlSeat, ()> for State {
        fn event(
            _: &mut Self,
            seat: &WlSeat,
            event: wl_seat::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let wl_seat::Event::Capabilities {
                capabilities: WEnum::Value(caps),
            } = event
                && caps.contains(wl_seat::Capability::Keyboard)
            {
                seat.get_keyboard(qh, ());
            }
        }
    }

    impl Dispatch<WlKeyboard, ()> for State {
        fn event(
            state: &mut Self,
            _: &WlKeyboard,
            event: wl_keyboard::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_keyboard::Event::Keymap { format, fd, size } = event
                && format == WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)
            {
                let fd: OwnedFd = fd;
                let mut file = File::from(fd);
                let mut bytes = vec![0u8; size as usize];
                if file.read_exact(&mut bytes).is_ok() {
                    state.seat_keymap = Some(bytes);
                }
            }
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
            if let wl_output::Event::Done = event {
                state.outputs.entry(*name).or_default().done = true;
                state.outputs_changed = true;
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
                zxdg_output_v1::Event::Done => {
                    o.done = true;
                    state.outputs_changed = true;
                }
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
        ZwlrVirtualPointerManagerV1,
        ZwlrVirtualPointerV1,
        ZwpVirtualKeyboardManagerV1,
        ZwpVirtualKeyboardV1,
        ZxdgOutputManagerV1,
    );

    pub fn run() -> Result<(), Error> {
        let conn = Connection::connect_to_env().map_err(|e| Error::Io(format!("wayland: {e}")))?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)
            .map_err(|e| Error::Io(format!("wayland registry: {e}")))?;
        let qh = queue.handle();
        let pointer_manager: ZwlrVirtualPointerManagerV1 = globals
            .bind(&qh, 1..=2, ())
            .map_err(|e| Error::Io(format!("no wlr-virtual-pointer: {e}")))?;
        let keyboard_manager: ZwpVirtualKeyboardManagerV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| Error::Io(format!("no virtual-keyboard: {e}")))?;
        let xdg_output_manager: ZxdgOutputManagerV1 = globals
            .bind(&qh, 1..=3, ())
            .map_err(|e| Error::Io(format!("no xdg-output: {e}")))?;
        let registry = globals.registry();
        let seat: WlSeat = globals
            .contents()
            .with_list(|list| {
                list.iter()
                    .find(|g| g.interface == WlSeat::interface().name)
                    .map(|g| registry.bind(g.name, g.version.min(7), &qh, ()))
            })
            .ok_or_else(|| Error::Io("no Wayland seat".to_owned()))?;
        let outputs: Vec<(u32, WlOutput)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == WlOutput::interface().name)
                .map(|g| (g.name, registry.bind(g.name, g.version.min(4), &qh, g.name)))
                .collect()
        });
        let pointer = pointer_manager.create_virtual_pointer(Some(&seat), &qh, ());
        let mut state = State {
            started: Instant::now(),
            pointer,
            keyboard_manager,
            seat,
            keyboard: None,
            seat_keymap: None,
            outputs: HashMap::new(),
            xdg_outputs: HashMap::new(),
            held: Vec::new(),
            reported: None,
            outputs_changed: false,
        };
        for (name, output) in &outputs {
            let xdg = xdg_output_manager.get_xdg_output(output, &qh, *name);
            state.xdg_outputs.insert(*name, xdg);
        }
        // Two round trips: the seat's capabilities, then its keymap and the
        // outputs' geometry.
        for _ in 0..2 {
            queue
                .roundtrip(&mut state)
                .map_err(|e| Error::Io(format!("wayland roundtrip: {e}")))?;
        }
        let mut stdout = std::io::stdout();
        let report = |state: &mut State, out: &mut std::io::Stdout| -> Result<(), Error> {
            let screen = state.screen();
            if state.reported != Some(screen) {
                let mut msg = HelperMessage::new(types::READY);
                msg.width = Some(screen.0);
                msg.height = Some(screen.1);
                write_frame(out, &msg)?;
                state.reported = Some(screen);
            }
            state.outputs_changed = false;
            Ok(())
        };
        report(&mut state, &mut stdout)?;
        let mut stdin = std::io::stdin();
        while let Some(msg) = read_frame(&mut stdin)? {
            match msg.message_type.as_str() {
                types::BEGIN => state.begin(msg.x.unwrap_or(0.5), msg.y.unwrap_or(0.5)),
                types::EVENT => {
                    if let Some(event) = &msg.event {
                        state.inject(&qh, event);
                    }
                }
                types::END => state.end(&qh),
                _ => {}
            }
            queue
                .roundtrip(&mut state)
                .map_err(|e| Error::Io(format!("wayland roundtrip: {e}")))?;
            if state.outputs_changed {
                report(&mut state, &mut stdout)?;
            }
        }
        // The daemon went away: nothing stays pressed.
        state.end(&qh);
        let _ = queue.roundtrip(&mut state);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Capture: the InputCapture portal and libei.

/// One pointer barrier the daemon set: which edge it stands for and on
/// which zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeBarrier {
    /// The portal's non-zero id.
    pub id: u32,
    /// The edge of this device's layout.
    pub edge: Edge,
    /// The zone it lies on.
    pub zone: Zone,
    /// Its ends, as the portal wants them.
    pub line: (i32, i32, i32, i32),
}

/// The barriers for `edges` on the outer boundary of `zones`: for each edge,
/// one barrier per zone whose edge lies on the layout's extreme, so a pointer
/// leaving the layout that way crosses one of them.
#[must_use]
pub fn barriers_for(edges: &[Edge], zones: &[Zone]) -> Vec<EdgeBarrier> {
    if zones.is_empty() {
        return Vec::new();
    }
    let left = zones.iter().map(|z| z.x).min().unwrap_or(0);
    let top = zones.iter().map(|z| z.y).min().unwrap_or(0);
    let right = zones
        .iter()
        .map(|z| z.x + z.width as i32)
        .max()
        .unwrap_or(0);
    let bottom = zones
        .iter()
        .map(|z| z.y + z.height as i32)
        .max()
        .unwrap_or(0);
    let mut out = Vec::new();
    let mut id = 1u32;
    for edge in edges {
        for zone in zones {
            let (zx2, zy2) = (
                zone.x + zone.width as i32 - 1,
                zone.y + zone.height as i32 - 1,
            );
            // The portal takes a barrier only on a zone's boundary: the left
            // and top edges at x and y, the right and bottom at x + width and
            // y + height, one past the last pixel (xdg-desktop-portal-hyprland
            // refuses a barrier on the last pixel as inside the output).
            let line = match edge {
                Edge::Left if zone.x == left => (zone.x, zone.y, zone.x, zy2),
                Edge::Right if zone.x + zone.width as i32 == right => (right, zone.y, right, zy2),
                Edge::Top if zone.y == top => (zone.x, zone.y, zx2, zone.y),
                Edge::Bottom if zone.y + zone.height as i32 == bottom => {
                    (zone.x, bottom, zx2, bottom)
                }
                _ => continue,
            };
            out.push(EdgeBarrier {
                id,
                edge: *edge,
                zone: *zone,
                line,
            });
            id += 1;
        }
    }
    out
}

/// Where along its edge a barrier was crossed, from the cursor position the
/// portal reported, as a fraction of the whole layout's edge.
#[must_use]
pub fn hit_at(barrier: &EdgeBarrier, zones: &[Zone], cursor: (f64, f64)) -> f64 {
    edge_fraction(barrier.edge, zones, cursor)
}

struct CaptureState {
    armed: Vec<Edge>,
    zones: Vec<Zone>,
    zone_set: u32,
    barriers: Vec<EdgeBarrier>,
    activation: Option<u32>,
}

/// The Linux capture: the InputCapture portal's barriers and a libei
/// receiver. Implements [`Capture`]; `provides` once the session exists.
pub struct LinuxCapture {
    handle: tokio::runtime::Handle,
    events: broadcast::Sender<CaptureEvent>,
    state: Arc<Mutex<CaptureState>>,
    available: Arc<AtomicBool>,
    portal: Arc<tokio::sync::Mutex<Option<Portal>>>,
}

struct Portal {
    proxy: Arc<ashpd::desktop::input_capture::InputCapture>,
    session: ashpd::desktop::Session<ashpd::desktop::input_capture::InputCapture>,
}

impl LinuxCapture {
    /// Starts the capture: creates the portal session and the libei
    /// connection in the background. Called within the daemon's runtime.
    #[must_use]
    pub fn start() -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        let this = Arc::new(Self {
            handle: tokio::runtime::Handle::current(),
            events,
            state: Arc::new(Mutex::new(CaptureState {
                armed: Vec::new(),
                zones: Vec::new(),
                zone_set: 0,
                barriers: Vec::new(),
                activation: None,
            })),
            available: Arc::new(AtomicBool::new(false)),
            portal: Arc::new(tokio::sync::Mutex::new(None)),
        });
        let starter = this.clone();
        this.handle.spawn(async move {
            // Not before the session: an early call would start the portal
            // without the session's environment (LP-157).
            crate::portal_gate::wait_for_session().await;
            let mut told = false;
            loop {
                match starter.connect().await {
                    Ok(()) => return,
                    Err(e) if !told => {
                        tracing::warn!(%e, "input capture is not available (no InputCapture portal?); trying again every half minute");
                        told = true;
                    }
                    Err(e) => tracing::debug!(%e, "input capture still not available"),
                }
                tokio::time::sleep(PORTAL_RETRY).await;
            }
        });
        this
    }

    async fn connect(self: &Arc<Self>) -> Result<(), Error> {
        use ashpd::desktop::input_capture::{
            Capabilities, ConnectToEISOptions, CreateSessionOptions, InputCapture,
        };
        let portal_err = |e: ashpd::Error| Error::Io(format!("InputCapture portal: {e}"));
        let proxy = Arc::new(InputCapture::new().await.map_err(portal_err)?);
        let (session, _caps) = proxy
            .create_session(
                None,
                CreateSessionOptions::default()
                    .set_capabilities(Capabilities::Keyboard | Capabilities::Pointer),
            )
            .await
            .map_err(portal_err)?;
        let fd = proxy
            .connect_to_eis(&session, ConnectToEISOptions::default())
            .await
            .map_err(portal_err)?;
        *self.portal.lock().await = Some(Portal {
            proxy: proxy.clone(),
            session,
        });
        self.available.store(true, Ordering::SeqCst);
        tracing::info!("input capture session created through the InputCapture portal");

        // The libei receiver: its event stream is not `Send`, so it runs on
        // a thread of its own with a single-threaded runtime.
        let ei = self.clone();
        std::thread::Builder::new()
            .name("libei-receiver".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => {
                        if let Err(e) = runtime.block_on(ei.receive(fd)) {
                            tracing::warn!(%e, "libei receiver ended");
                        }
                    }
                    Err(e) => tracing::warn!(%e, "no runtime for the libei receiver"),
                }
                ei.available.store(false, Ordering::SeqCst);
            })
            .map_err(|e| Error::Io(format!("libei thread: {e}")))?;

        // The portal's signals, each on a task that owns its stream.
        let (this, portal) = (self.clone(), proxy.clone());
        self.handle.spawn(async move {
            use futures_util::StreamExt as _;
            let Ok(mut activated) = portal.receive_activated().await else {
                return;
            };
            while let Some(a) = activated.next().await {
                let (barrier, zones) = {
                    let state = lock(&this.state);
                    let barrier = a.barrier_id().and_then(|b| match b {
                        ashpd::desktop::input_capture::ActivatedBarrier::Barrier(id) => {
                            state.barriers.iter().find(|x| x.id == id.get()).copied()
                        }
                        _ => None,
                    });
                    (barrier, state.zones.clone())
                };
                lock(&this.state).activation = a.activation_id();
                let cursor = a
                    .cursor_position()
                    .map_or((0.0, 0.0), |(x, y)| (f64::from(x), f64::from(y)));
                if let Some(barrier) = barrier {
                    let at = hit_at(&barrier, &zones, cursor);
                    let _ = this.events.send(CaptureEvent::EdgeHit {
                        edge: barrier.edge,
                        at,
                    });
                } else {
                    tracing::debug!("input capture activated without a known barrier");
                }
            }
        });
        let (this, portal) = (self.clone(), proxy.clone());
        self.handle.spawn(async move {
            use futures_util::StreamExt as _;
            let Ok(mut deactivated) = portal.receive_deactivated().await else {
                return;
            };
            while deactivated.next().await.is_some() {
                lock(&this.state).activation = None;
                let _ = this.events.send(CaptureEvent::Lost);
            }
        });
        let (this, portal) = (self.clone(), proxy.clone());
        self.handle.spawn(async move {
            use futures_util::StreamExt as _;
            let Ok(mut disabled) = portal.receive_disabled().await else {
                return;
            };
            while disabled.next().await.is_some() {
                let _ = this.events.send(CaptureEvent::Lost);
                // The compositor disabled the session: arm again.
                this.rearm();
            }
        });
        let (this, portal) = (self.clone(), proxy);
        self.handle.spawn(async move {
            use futures_util::StreamExt as _;
            let Ok(mut zones_changed) = portal.receive_zones_changed().await else {
                return;
            };
            while zones_changed.next().await.is_some() {
                this.rearm();
            }
        });
        self.rearm();
        Ok(())
    }

    /// Reads the zones and sets the barriers for the armed edges, then
    /// enables the session when any barrier stands.
    fn rearm(self: &Arc<Self>) {
        let this = self.clone();
        self.handle.spawn(async move {
            if let Err(e) = this.rearm_inner().await {
                tracing::warn!(%e, "cannot set the input capture barriers");
            }
        });
    }

    async fn rearm_inner(&self) -> Result<(), Error> {
        use ashpd::desktop::input_capture::{
            Barrier, DisableOptions, EnableOptions, GetZonesOptions, SetPointerBarriersOptions,
        };
        let portal_err = |e: ashpd::Error| Error::Io(format!("InputCapture portal: {e}"));
        let guard = self.portal.lock().await;
        let Some(portal) = guard.as_ref() else {
            return Ok(());
        };
        let zones = portal
            .proxy
            .zones(&portal.session, GetZonesOptions::default())
            .await
            .map_err(portal_err)?
            .response()
            .map_err(portal_err)?;
        let list: Vec<Zone> = zones
            .regions()
            .iter()
            .map(|r| Zone {
                x: r.x_offset(),
                y: r.y_offset(),
                width: r.width(),
                height: r.height(),
            })
            .collect();
        let (edges, barriers) = {
            let mut state = lock(&self.state);
            state.zones.clone_from(&list);
            state.zone_set = zones.zone_set();
            state.barriers = barriers_for(&state.armed, &list);
            (state.armed.clone(), state.barriers.clone())
        };
        let portal_barriers: Vec<Barrier> = barriers
            .iter()
            .filter_map(|b| std::num::NonZeroU32::new(b.id).map(|id| Barrier::new(id, b.line)))
            .collect();
        let response = portal
            .proxy
            .set_pointer_barriers(
                &portal.session,
                &portal_barriers,
                zones.zone_set(),
                SetPointerBarriersOptions::default(),
            )
            .await
            .map_err(portal_err)?
            .response()
            .map_err(portal_err)?;
        if !response.failed_barriers().is_empty() {
            tracing::warn!(failed = ?response.failed_barriers(), "the compositor refused some barriers");
        }
        if portal_barriers.is_empty() {
            let _ = portal
                .proxy
                .disable(&portal.session, DisableOptions::default())
                .await;
        } else {
            portal
                .proxy
                .enable(&portal.session, EnableOptions::default())
                .await
                .map_err(portal_err)?;
        }
        tracing::info!(
            ?edges,
            zones = list.len(),
            barriers = barriers.len(),
            "input capture armed"
        );
        Ok(())
    }

    /// Receives the captured input over libei and turns it into events.
    async fn receive(&self, fd: OwnedFd) -> Result<(), Error> {
        use futures_util::StreamExt as _;
        use reis::ei;
        use reis::event::{DeviceCapability, EiEvent};
        let stream = std::os::unix::net::UnixStream::from(fd);
        stream
            .set_nonblocking(true)
            .map_err(|e| Error::Io(format!("eis socket: {e}")))?;
        let context = ei::Context::new(stream).map_err(|e| Error::Io(format!("libei: {e}")))?;
        let (_connection, mut events) = context
            .handshake_tokio("ligatured", ei::handshake::ContextType::Receiver)
            .await
            .map_err(|e| Error::Io(format!("libei handshake: {e}")))?;
        while let Some(event) = events.next().await {
            let event = event.map_err(|e| Error::Io(format!("libei: {e}")))?;
            let out = match event {
                EiEvent::SeatAdded(e) => {
                    e.seat.bind_capabilities(
                        DeviceCapability::Pointer
                            | DeviceCapability::Keyboard
                            | DeviceCapability::Button
                            | DeviceCapability::Scroll,
                    );
                    let _ = context.flush();
                    continue;
                }
                EiEvent::PointerMotion(m) => InputEvent::Motion {
                    dx: f64::from(m.dx),
                    dy: f64::from(m.dy),
                },
                EiEvent::Button(b) => InputEvent::Button {
                    code: u16::try_from(b.button).unwrap_or(0),
                    pressed: matches!(b.state, ei::button::ButtonState::Press),
                },
                EiEvent::ScrollDelta(s) => InputEvent::Scroll {
                    dx: f64::from(s.dx),
                    dy: f64::from(s.dy),
                    discrete: false,
                },
                EiEvent::ScrollDiscrete(s) => InputEvent::Scroll {
                    dx: f64::from(s.discrete_dx) / EI_DETENT,
                    dy: f64::from(s.discrete_dy) / EI_DETENT,
                    discrete: true,
                },
                EiEvent::KeyboardKey(k) => InputEvent::Key {
                    code: u16::try_from(k.key).unwrap_or(0),
                    pressed: matches!(k.state, ei::keyboard::KeyState::Press),
                },
                EiEvent::Disconnected(_) => break,
                _ => continue,
            };
            let _ = self.events.send(CaptureEvent::Input(out));
        }
        Ok(())
    }
}

impl Capture for LinuxCapture {
    fn provides(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    fn screen(&self) -> (u32, u32) {
        size(&lock(&self.state).zones)
    }

    fn arm(&self, edges: &[Edge]) {
        lock(&self.state).armed = edges.to_vec();
        let this = Arc::new(LinuxCaptureRef(
            self.state.clone(),
            self.portal.clone(),
            self.handle.clone(),
        ));
        this.rearm();
    }

    fn take(&self, _edge: Edge) -> Result<(), Error> {
        // The compositor already captures: `Activated` is what brought us here.
        Ok(())
    }

    fn release(&self, back_at: Option<(Edge, f64)>) {
        use ashpd::desktop::input_capture::ReleaseOptions;
        let (activation, zones) = {
            let state = lock(&self.state);
            (state.activation, state.zones.clone())
        };
        let position = back_at.map(|(edge, at)| back_position(edge, at, &zones));
        let portal = self.portal.clone();
        self.handle.spawn(async move {
            let guard = portal.lock().await;
            let Some(portal) = guard.as_ref() else { return };
            let options = ReleaseOptions::default()
                .set_activation_id(activation)
                .set_cursor_position(position);
            if let Err(e) = portal.proxy.release(&portal.session, options).await {
                tracing::debug!(%e, "input capture release");
            }
        });
    }

    fn subscribe(&self) -> broadcast::Receiver<CaptureEvent> {
        self.events.subscribe()
    }
}

/// The pieces `rearm` needs, so `arm` (a `&self` trait method) can spawn it
/// without an `Arc<LinuxCapture>`.
struct LinuxCaptureRef(
    Arc<Mutex<CaptureState>>,
    Arc<tokio::sync::Mutex<Option<Portal>>>,
    tokio::runtime::Handle,
);

impl LinuxCaptureRef {
    fn rearm(self: Arc<Self>) {
        let handle = self.2.clone();
        handle.spawn(async move {
            let shadow = LinuxCapture {
                handle: self.2.clone(),
                events: broadcast::channel(1).0,
                state: self.0.clone(),
                available: Arc::new(AtomicBool::new(true)),
                portal: self.1.clone(),
            };
            if let Err(e) = shadow.rearm_inner().await {
                tracing::warn!(%e, "cannot set the input capture barriers");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(x: i32, y: i32, w: u32, h: u32) -> Zone {
        Zone {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn barriers_lie_on_the_outer_boundary_of_the_zones() {
        let zones = [zone(0, 0, 1920, 1080), zone(1920, 0, 2560, 1440)];
        let b = barriers_for(&[Edge::Right, Edge::Left, Edge::Top], &zones);
        let right: Vec<_> = b.iter().filter(|b| b.edge == Edge::Right).collect();
        assert_eq!(
            right.len(),
            1,
            "only the second zone's right edge is the layout's"
        );
        // Hyprland's portal takes a barrier only on a zone's boundary: the
        // right edge is at x + width, not on the last pixel.
        assert_eq!(right[0].line, (4480, 0, 4480, 1439));
        let left: Vec<_> = b.iter().filter(|b| b.edge == Edge::Left).collect();
        assert_eq!(left[0].line, (0, 0, 0, 1079));
        let top: Vec<_> = b.iter().filter(|b| b.edge == Edge::Top).collect();
        assert_eq!(top.len(), 2, "both zones touch the top");
        assert_eq!(top[0].line, (0, 0, 1919, 0));
        let ids: Vec<u32> = b.iter().map(|b| b.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
        // The bottom edge likewise at y + height, on the taller zone only.
        let bottom = barriers_for(&[Edge::Bottom], &zones);
        assert_eq!(bottom.len(), 1);
        assert_eq!(bottom[0].line, (1920, 1440, 4479, 1440));
        assert!(barriers_for(&[Edge::Right], &[]).is_empty());
    }

    #[test]
    fn a_hit_is_a_fraction_of_the_layout_s_edge_and_the_way_back_is_just_inside() {
        let zones = [zone(0, 0, 1920, 1080), zone(1920, 0, 2560, 1440)];
        let right = barriers_for(&[Edge::Right], &zones).remove(0);
        assert!((hit_at(&right, &zones, (4479.0, 720.0)) - 0.5).abs() < 1e-9);
        let top = barriers_for(&[Edge::Top], &zones).remove(0);
        assert!((hit_at(&top, &zones, (2240.0, 0.0)) - 0.5).abs() < 1e-9);
        assert_eq!(back_position(Edge::Right, 0.5, &zones), (4477.0, 720.0));
        assert_eq!(back_position(Edge::Left, 0.0, &zones), (2.0, 0.0));
        assert_eq!(back_position(Edge::Bottom, 1.0, &zones), (4480.0, 1437.0));
    }

    #[test]
    fn the_text_keymap_names_each_character_once_as_a_unicode_keysym() {
        let (keymap, codes) = text_keymap("héllo");
        assert_eq!(codes.len(), 4, "l appears twice");
        assert_eq!(codes[&'h'], 1);
        assert!(keymap.contains("<K1> = 9;"));
        assert!(keymap.contains("key <K2> { [ U00E9 ] };"));
        assert!(keymap.starts_with("xkb_keymap {"));
        assert!(keymap.contains("xkb_symbols"));
    }
}
