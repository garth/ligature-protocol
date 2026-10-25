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

/// The core modifier bits of an XKB modifier mask: the first eight modifiers
/// have these indices in every keymap.
pub mod mod_bits {
    /// Shift.
    pub const SHIFT: u32 = 1;
    /// Lock (Caps Lock).
    pub const LOCK: u32 = 1 << 1;
    /// Control.
    pub const CONTROL: u32 = 1 << 2;
    /// Mod1, Alt.
    pub const MOD1: u32 = 1 << 3;
    /// Mod2, Num Lock.
    pub const MOD2: u32 = 1 << 4;
    /// Mod4, Super.
    pub const MOD4: u32 = 1 << 6;
    /// Mod5, the third level (AltGr).
    pub const MOD5: u32 = 1 << 7;
}

/// What a modifier key does to the modifier state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModRole {
    /// Sets these bits while it is held.
    Held(u32),
    /// Toggles these bits, locked, each time it is pressed.
    Locks(u32),
}

/// The role of a key from the keysym on its first level.
fn role_of(keysym: &str) -> Option<ModRole> {
    use mod_bits::{CONTROL, LOCK, MOD1, MOD2, MOD4, MOD5, SHIFT};
    Some(match keysym {
        "Shift_L" | "Shift_R" => ModRole::Held(SHIFT),
        "Control_L" | "Control_R" => ModRole::Held(CONTROL),
        "Alt_L" | "Alt_R" | "Meta_L" | "Meta_R" => ModRole::Held(MOD1),
        "Super_L" | "Super_R" | "Hyper_L" | "Hyper_R" => ModRole::Held(MOD4),
        "ISO_Level3_Shift" | "Mode_switch" => ModRole::Held(MOD5),
        "Caps_Lock" => ModRole::Locks(LOCK),
        "Num_Lock" => ModRole::Locks(MOD2),
        _ => return None,
    })
}

/// The modifier keys of a keymap and their roles, by evdev code, read from
/// what each key produces on its first level (the keymap's own
/// `modifier_map` is not used: as a compositor serialises it, an option such
/// as `shift:both_capslock_cancel` puts Left Shift in Lock alone). A keymap
/// that cannot be read gives the US roles.
#[must_use]
pub fn modifier_roles(keymap: &str) -> HashMap<u16, ModRole> {
    // `<NAME> = keycode;` in the keycodes section; the evdev code is the
    // keycode less 8.
    let mut codes: HashMap<&str, u16> = HashMap::new();
    for line in keymap.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix('<') else {
            continue;
        };
        let Some((name, rest)) = rest.split_once('>') else {
            continue;
        };
        let Some(number) = rest.trim().strip_prefix('=') else {
            continue;
        };
        if let Ok(code) = number.trim().trim_end_matches(';').trim().parse::<u16>()
            && code >= 8
        {
            codes.insert(name, code - 8);
        }
    }
    let mut roles = HashMap::new();
    let mut rest = keymap;
    while let Some(at) = rest.find("key <") {
        let after = &rest[at + 5..];
        let Some(close) = after.find('>') else { break };
        let name = &after[..close];
        let body = &after[close + 1..];
        let block = &body[..body.find('}').unwrap_or(body.len())];
        // `{ [ a, A ] }`, or `symbols[1]= [ a, A ]` in a longer block.
        let from = block
            .find("symbols[1]")
            .and_then(|p| block[p..].find('=').map(|e| p + e + 1))
            .unwrap_or(0);
        if let Some(open) = block[from..].find('[') {
            let keysym = block[from + open + 1..]
                .split([',', ']'])
                .next()
                .unwrap_or("")
                .trim();
            if let (Some(code), Some(role)) = (codes.get(name), role_of(keysym)) {
                roles.insert(*code, role);
            }
        }
        rest = body;
    }
    if roles.is_empty() {
        us_modifier_roles()
    } else {
        roles
    }
}

/// The modifier keys of the US keymap by evdev code.
#[must_use]
pub fn us_modifier_roles() -> HashMap<u16, ModRole> {
    use mod_bits::{CONTROL, LOCK, MOD1, MOD2, MOD4};
    HashMap::from([
        (42, ModRole::Held(mod_bits::SHIFT)),
        (54, ModRole::Held(mod_bits::SHIFT)),
        (29, ModRole::Held(CONTROL)),
        (97, ModRole::Held(CONTROL)),
        (56, ModRole::Held(MOD1)),
        (100, ModRole::Held(MOD1)),
        (125, ModRole::Held(MOD4)),
        (126, ModRole::Held(MOD4)),
        (58, ModRole::Locks(LOCK)),
        (69, ModRole::Locks(MOD2)),
    ])
}

/// The modifier state of a virtual keyboard, which the client must report
/// itself (`zwp_virtual_keyboard_v1.modifiers`): the compositor takes it
/// from that message alone, not from the key events, and merges it with the
/// other keyboards' for the applications.
#[derive(Debug, Default)]
pub struct ModState {
    roles: HashMap<u16, ModRole>,
    held: Vec<u16>,
    locked: u32,
}

impl ModState {
    /// A state for a keymap's modifier roles, nothing held or locked.
    #[must_use]
    pub fn new(roles: HashMap<u16, ModRole>) -> Self {
        Self {
            roles,
            held: Vec::new(),
            locked: 0,
        }
    }

    /// A key went down or up: the depressed and locked masks to report, if
    /// it is a modifier key.
    pub fn key(&mut self, code: u16, pressed: bool) -> Option<(u32, u32)> {
        match *self.roles.get(&code)? {
            ModRole::Held(_) => {
                self.held.retain(|c| *c != code);
                if pressed {
                    self.held.push(code);
                }
            }
            ModRole::Locks(bits) => {
                if pressed {
                    self.locked ^= bits;
                }
            }
        }
        Some((self.depressed(), self.locked))
    }

    /// Toggles a lock (Caps Lock's or Num Lock's bits), for a lock key sent
    /// by its meaning: the depressed and locked masks to report.
    pub fn toggle(&mut self, bits: u32) -> (u32, u32) {
        self.locked ^= bits;
        (self.depressed(), self.locked)
    }

    /// The locked bits.
    #[must_use]
    pub const fn locked(&self) -> u32 {
        self.locked
    }

    /// The bits of the modifier keys held.
    #[must_use]
    pub fn depressed(&self) -> u32 {
        self.held
            .iter()
            .filter_map(|c| match self.roles.get(c) {
                Some(ModRole::Held(bits)) => Some(*bits),
                _ => None,
            })
            .fold(0, |a, b| a | b)
    }
}

/// Which key of a keymap types a keysym, and with which modifiers: the
/// driven side of ADR 0043, after Synergy's key map (`KeyMap::findBestKey`
/// in Input Leap), which picks the entry needing the fewest modifiers.
pub struct KeyFinder {
    keymap: xkbcommon::xkb::Keymap,
    lock_bit: u32,
    found: HashMap<u32, Option<(u16, u32)>>,
}

impl std::fmt::Debug for KeyFinder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyFinder")
    }
}

impl KeyFinder {
    /// The finder for a keymap's text, `None` if it does not compile.
    #[must_use]
    pub fn new(keymap: &str) -> Option<Self> {
        use xkbcommon::xkb;
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_string(
            &context,
            keymap.trim_end_matches('\0').to_owned(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )?;
        Some(Self::from_keymap(keymap))
    }

    fn from_keymap(keymap: xkbcommon::xkb::Keymap) -> Self {
        let lock = keymap.mod_get_index(xkbcommon::xkb::MOD_NAME_CAPS);
        let lock_bit = 1u32.checked_shl(lock).unwrap_or(0);
        Self {
            keymap,
            lock_bit,
            found: HashMap::new(),
        }
    }

    /// The key (evdev code) and modifier mask that type `keysym` on the first
    /// layout: the one needing the fewest modifiers, Shift before any other,
    /// never Caps Lock.
    pub fn find(&mut self, keysym: u32) -> Option<(u16, u32)> {
        if let Some(found) = self.found.get(&keysym) {
            return *found;
        }
        let found = self.search(keysym);
        self.found.insert(keysym, found);
        found
    }

    fn search(&self, keysym: u32) -> Option<(u16, u32)> {
        use xkbcommon::xkb::Keycode;
        let mut best: Option<(u32, u16, u32)> = None;
        let (min, max) = (
            self.keymap.min_keycode().raw(),
            self.keymap.max_keycode().raw(),
        );
        for raw in min..=max {
            let key = Keycode::new(raw);
            let Ok(evdev) = u16::try_from(raw.saturating_sub(8)) else {
                continue;
            };
            if raw < 8 {
                continue;
            }
            for level in 0..self.keymap.num_levels_for_key(key, 0) {
                let syms = self.keymap.key_get_syms_by_level(key, 0, level);
                if !syms.iter().any(|s| s.raw() == keysym) {
                    continue;
                }
                let mut masks = [0u32; 16];
                let n = self
                    .keymap
                    .key_get_mods_for_level(key, 0, level, &mut masks);
                for mask in masks
                    .iter()
                    .take(n)
                    .copied()
                    .filter(|m| m & self.lock_bit == 0)
                {
                    // Shift is the ordinary second level; any other modifier
                    // (AltGr) counts double, so `@` on a British layout is
                    // Shift and the quote key, not AltGr and Q.
                    let cost = (mask & 1) + 2 * (mask & !1).count_ones();
                    if best.is_none_or(|(c, _, _)| cost < c) {
                        best = Some((cost, evdev, mask));
                    }
                }
            }
        }
        best.map(|(_, evdev, mask)| (evdev, mask))
    }
}

/// The X11 keysym a key ID stands for: a character's, or a named key's.
#[must_use]
pub fn keysym_of(id: lp_core::input::keyid::KeyId) -> Option<u32> {
    use lp_core::input::keyid;
    if let Some(c) = keyid::character(id) {
        let sym = xkbcommon::xkb::utf32_to_keysym(u32::from(c)).raw();
        return (sym != 0).then_some(sym);
    }
    keyid::keysym_of_named(id)
}

/// What the keys of the capture's keyboard mean, for a controller on Linux
/// (ADR 0043): the keymap libei hands the capture, its modifier state, and
/// the locale's compose table, so a dead key is composed here and the key
/// that completes it carries the composed character.
pub struct KeyMeaning {
    keymap: xkbcommon::xkb::Keymap,
    state: xkbcommon::xkb::State,
    compose: Option<xkbcommon::xkb::compose::State>,
    /// The modifier indices of the mask bits, in [`keyid::mask`] order.
    mods: Vec<(xkbcommon::xkb::ModIndex, u32)>,
    /// Modifiers that do not choose a character: Control, Alt, Super, Meta.
    command: u32,
}

impl std::fmt::Debug for KeyMeaning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyMeaning")
    }
}

/// What a captured key press means: `None` for a dead key's press, which
/// sends nothing, else the ID (if any) and the modifier mask.
pub type Meaning = Option<(Option<lp_core::input::keyid::KeyId>, u32)>;

impl KeyMeaning {
    /// The meanings of a keymap's text, composed with `locale`'s table when
    /// it has one; `None` if the keymap does not compile.
    #[must_use]
    pub fn new(keymap: &str, locale: Option<&str>) -> Option<Self> {
        use lp_core::input::keyid::mask;
        use xkbcommon::xkb;
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_string(
            &context,
            keymap.trim_end_matches('\0').to_owned(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )?;
        let compose = locale.and_then(|locale| {
            let table = xkb::compose::Table::new_from_locale(
                &context,
                std::ffi::OsStr::new(locale),
                xkb::compose::COMPILE_NO_FLAGS,
            )
            .ok()?;
            Some(xkb::compose::State::new(
                &table,
                xkb::compose::STATE_NO_FLAGS,
            ))
        });
        let mods: Vec<_> = [
            (xkb::MOD_NAME_SHIFT, mask::SHIFT),
            (xkb::MOD_NAME_CTRL, mask::CONTROL),
            (xkb::MOD_NAME_ALT, mask::ALT),
            (xkb::MOD_NAME_LOGO, mask::SUPER),
            ("Mod5", mask::ALT_GR),
            (xkb::MOD_NAME_CAPS, mask::CAPS_LOCK),
            (xkb::MOD_NAME_NUM, mask::NUM_LOCK),
        ]
        .into_iter()
        .map(|(name, bit)| (keymap.mod_get_index(name), bit))
        .filter(|(index, _)| *index != xkb::MOD_INVALID)
        .collect();
        let command = mods
            .iter()
            .filter(|(_, bit)| bit & mask::COMMAND != 0)
            .fold(0, |a, (index, _)| a | (1 << index));
        let state = xkb::State::new(&keymap);
        Some(Self {
            keymap,
            state,
            compose,
            mods,
            command,
        })
    }

    /// Takes the modifier state the compositor reports.
    pub fn set_mods(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        self.state
            .update_mask(depressed, latched, locked, 0, 0, group);
    }

    /// The meaning of a key event by evdev code, and the state moved on by it.
    /// A release carries no ID (ADR 0043 decision 1).
    pub fn key(&mut self, code: u16, pressed: bool) -> Meaning {
        use lp_core::input::keyid;
        use xkbcommon::xkb::{self, Keycode, compose::Status};
        let key = Keycode::new(u32::from(code) + 8);
        let held = self.mask();
        if !pressed {
            self.state.update_key(key, xkb::KeyDirection::Up);
            return Some((None, self.mask()));
        }
        // What the key types with Shift, Caps Lock and AltGr, without the
        // modifiers that choose a shortcut rather than a character.
        let (d, l, k, g) = (
            self.state.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            self.state.serialize_mods(xkb::STATE_MODS_LATCHED),
            self.state.serialize_mods(xkb::STATE_MODS_LOCKED),
            self.state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
        );
        let c = !self.command;
        let mut plain = xkb::State::new(&self.keymap);
        plain.update_mask(d & c, l & c, k & c, 0, 0, g);
        let sym = plain.key_get_one_sym(key);
        let shortcut = held & keyid::mask::COMMAND != 0;
        let mut composed = None;
        if let Some(compose) = self.compose.as_mut().filter(|_| !shortcut)
            && compose.feed(sym) == xkb::compose::FeedResult::Accepted
        {
            match compose.status() {
                Status::Composing | Status::Cancelled => {
                    if compose.status() == Status::Cancelled {
                        compose.reset();
                    }
                    self.state.update_key(key, xkb::KeyDirection::Down);
                    return None;
                }
                Status::Composed => {
                    let text = compose.utf8().unwrap_or_default();
                    let mut chars = text.chars();
                    if let (Some(c), None) = (chars.next(), chars.next()) {
                        composed = Some(u32::from(c));
                    }
                    compose.reset();
                }
                Status::Nothing => {}
            }
        }
        let id = composed
            .or_else(|| keyid::named_from_keysym(sym.raw()))
            .or_else(|| {
                char::from_u32(plain.key_get_utf32(key))
                    .filter(|c| !c.is_control())
                    .map(u32::from)
            });
        self.state.update_key(key, xkb::KeyDirection::Down);
        Some((id, held))
    }

    /// The controller's modifier mask (ADR 0043) from the xkb state.
    fn mask(&self) -> u32 {
        use xkbcommon::xkb;
        self.mods
            .iter()
            .filter(|(index, _)| {
                self.state
                    .mod_index_is_active(*index, xkb::STATE_MODS_EFFECTIVE)
            })
            .fold(0, |a, (_, bit)| a | bit)
    }
}

/// The text of a keymap libei handed over, if it is an XKB one.
fn read_keymap(keymap: &reis::event::Keymap) -> Option<String> {
    use std::os::unix::fs::FileExt as _;
    if keymap.type_ != reis::ei::keyboard::KeymapType::Xkb {
        return None;
    }
    let file = std::fs::File::from(keymap.fd.try_clone().ok()?);
    let mut bytes = vec![0; usize::try_from(keymap.size).ok()?];
    file.read_exact_at(&mut bytes, 0).ok()?;
    String::from_utf8(bytes).ok()
}

/// The locale whose compose table composes dead keys, as the environment
/// names it, else `C`.
fn locale() -> String {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| "C".to_owned())
}

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
    use lp_core::input::keyid;
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
        HelperMessage, KeyFinder, ModState, US_KEYMAP, WHEEL_STEP, keysym_of, mod_bits,
        modifier_roles, read_frame, text_keymap, types, write_frame,
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
        /// The virtual keyboard's modifier state, which it reports itself.
        mods: ModState,
        /// Which key of the seat keymap types a meaning (ADR 0043).
        finder: Option<KeyFinder>,
        /// The local key each physical key's press used, by the controller's
        /// code, so its release lifts it; `None` for a press typed as text.
        pressed_as: HashMap<u16, Option<u16>>,
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
                let text = String::from_utf8_lossy(&keymap);
                self.mods = ModState::new(modifier_roles(&text));
                self.finder = KeyFinder::new(&text);
                if self.finder.is_none() {
                    tracing::warn!("the seat keymap does not compile; keys go by position");
                }
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
                InputEvent::Key {
                    code,
                    pressed,
                    id,
                    mods,
                } => {
                    let kb = self.ensure_keyboard(qh).clone();
                    let handled = if *pressed {
                        id.is_some_and(|id| {
                            self.press_meaning(qh, &kb, t, *code, id, mods.unwrap_or(0))
                        })
                    } else if let Some(local) = self.pressed_as.remove(code) {
                        if let Some(local) = local {
                            self.key_local(&kb, t, local, false);
                        }
                        true
                    } else {
                        false
                    };
                    if !handled {
                        // By position: the controller sent no meaning, or
                        // this keymap has nothing for it.
                        self.key_local(&kb, t, *code, *pressed);
                    }
                    self.note_held(event);
                }
                InputEvent::Text { text } => self.type_text(qh, t, text),
            }
        }

        /// A key of the seat keymap, by evdev code, with the modifier state
        /// reported after it. The compositor takes a virtual keyboard's
        /// modifiers from that report alone, never from its keys: without it
        /// Shift shifts nothing, and with `shift:both_capslock_cancel` a
        /// Shift can lock Caps.
        fn key_local(&mut self, kb: &ZwpVirtualKeyboardV1, t: u32, code: u16, pressed: bool) {
            kb.key(t, u32::from(code), u32::from(pressed));
            if let Some((depressed, locked)) = self.mods.key(code, pressed) {
                kb.modifiers(depressed, 0, locked, 0);
            }
        }

        /// A press by its meaning (ADR 0043): returns whether it was placed.
        fn press_meaning(
            &mut self,
            qh: &QueueHandle<Self>,
            kb: &ZwpVirtualKeyboardV1,
            t: u32,
            code: u16,
            id: keyid::KeyId,
            mods: u32,
        ) -> bool {
            if keyid::is_lock(id) {
                // A lock toggles this keyboard's lock; no key is pressed.
                let bits = match id {
                    keyid::CAPS_LOCK => mod_bits::LOCK,
                    keyid::NUM_LOCK => mod_bits::MOD2,
                    _ => 0,
                };
                if bits != 0 {
                    let (depressed, locked) = self.mods.toggle(bits);
                    kb.modifiers(depressed, 0, locked, 0);
                }
                self.pressed_as.insert(code, None);
                return true;
            }
            let Some(keysym) = keysym_of(id) else {
                return false;
            };
            match self.finder.as_mut().and_then(|f| f.find(keysym)) {
                Some((local, level_mods)) => {
                    let command = mods & keyid::mask::COMMAND != 0;
                    if keyid::is_named(id) || command {
                        // A modifier, a named key or a shortcut: the key
                        // itself, under the modifiers held ("the right button
                        // not the right character", Synergy's mapCommandKey).
                        self.key_local(kb, t, local, true);
                    } else {
                        // A character: exactly the modifiers its level needs,
                        // no locks, for this press, then back.
                        kb.modifiers(level_mods, 0, 0, 0);
                        kb.key(t, u32::from(local), 1);
                        kb.modifiers(self.mods.depressed(), 0, self.mods.locked(), 0);
                    }
                    self.pressed_as.insert(code, Some(local));
                    true
                }
                None => match keyid::character(id) {
                    Some(c) => {
                        // A character this layout lacks, typed through a
                        // keymap of its own.
                        self.type_text(qh, t, &c.to_string());
                        self.pressed_as.insert(code, None);
                        true
                    }
                    None => false,
                },
            }
        }

        /// Types a string through a keyboard of its own, as `wtype` does: the
        /// compositor reads a virtual keyboard's keymap once, when it is set,
        /// so the seat-keymap keyboard stays as it is and this one lives for
        /// the string.
        fn type_text(&mut self, qh: &QueueHandle<Self>, t: u32, text: &str) {
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

        fn note_held(&mut self, event: &InputEvent) {
            let (lifted, pressed) = match event {
                InputEvent::Key { code, pressed, .. } => (InputEvent::key(*code, false), *pressed),
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
            mods: ModState::default(),
            finder: None,
            pressed_as: HashMap::new(),
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
        // What the keys mean (ADR 0043), once the keyboard's keymap arrives,
        // and the keys whose press was a dead key's and sent nothing.
        let mut meaning: Option<KeyMeaning> = None;
        let mut unsent: Vec<u16> = Vec::new();
        while let Some(event) = events.next().await {
            let event = event.map_err(|e| Error::Io(format!("libei: {e}")))?;
            let out = match event {
                EiEvent::DeviceAdded(e) => {
                    tracing::debug!(
                        name = e.device.name(),
                        keymap = e.device.keymap().is_some(),
                        "capture device"
                    );
                    if let Some(keymap) = e.device.keymap() {
                        meaning = read_keymap(keymap).and_then(|text| {
                            let found = KeyMeaning::new(&text, Some(&locale()));
                            if found.is_none() {
                                tracing::warn!(
                                    "the capture's keymap does not compile; keys go by position"
                                );
                            }
                            found
                        });
                    }
                    continue;
                }
                EiEvent::KeyboardModifiers(m) => {
                    if let Some(meaning) = meaning.as_mut() {
                        meaning.set_mods(m.depressed, m.latched, m.locked, m.group);
                    }
                    continue;
                }
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
                EiEvent::KeyboardKey(k) => {
                    let code = u16::try_from(k.key).unwrap_or(0);
                    let pressed = matches!(k.state, ei::keyboard::KeyState::Press);
                    let Some(meaning) = meaning.as_mut() else {
                        let _ = self
                            .events
                            .send(CaptureEvent::Input(InputEvent::key(code, pressed)));
                        continue;
                    };
                    match meaning.key(code, pressed) {
                        None => {
                            unsent.push(code);
                            continue;
                        }
                        Some(_) if !pressed && unsent.contains(&code) => {
                            unsent.retain(|c| *c != code);
                            continue;
                        }
                        Some((id, mods)) => InputEvent::Key {
                            code,
                            pressed,
                            id,
                            mods: Some(mods),
                        },
                    }
                }
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
    use lp_core::input::keyid;

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
    fn modifier_roles_come_from_what_each_key_produces() {
        // As this machine's compositor serialises its keymap with the
        // options compose:caps, shift:both_capslock_cancel and
        // altwin:swap_alt_win: Left Shift is in Lock's modifier_map, the Alt
        // and Win keys are swapped, Caps Lock composes.
        let keymap = "xkb_keymap {\n\
            xkb_keycodes \"evdev\" {\n\
            \t<LCTL>               = 37;\n\
            \t<LFSH>               = 50;\n\
            \t<RTSH>               = 62;\n\
            \t<LALT>               = 64;\n\
            \t<CAPS>               = 66;\n\
            \t<NMLK>               = 77;\n\
            \t<LWIN>               = 133;\n\
            \talias <ALGR>         = <RALT>;\n\
            };\n\
            xkb_symbols \"pc\" {\n\
            \tkey <LCTL>               {\t[       Control_L ] };\n\
            \tkey <LFSH>               {\n\
            \t\ttype= \"ALPHABETIC\",\n\
            \t\tsymbols[1]= [         Shift_L,       Caps_Lock ]\n\
            \t};\n\
            \tkey <RTSH>               {\n\
            \t\ttype= \"ALPHABETIC\",\n\
            \t\tsymbols[1]= [         Shift_R,       Caps_Lock ]\n\
            \t};\n\
            \tkey <CAPS>               {\n\
            \t\ttype= \"TWO_LEVEL\",\n\
            \t\tsymbols[1]= [       Multi_key,       Multi_key ]\n\
            \t};\n\
            \tkey <LALT>               {\n\
            \t\ttype= \"ONE_LEVEL\",\n\
            \t\tsymbols[1]= [         Super_L ]\n\
            \t};\n\
            \tkey <NMLK>               {\t[        Num_Lock ] };\n\
            \tkey <LWIN>               {\t[           Alt_L,          Meta_L ] };\n\
            \tmodifier_map Shift { <RTSH> };\n\
            \tmodifier_map Lock { <LFSH> };\n\
            };\n\
            };\n";
        let roles = modifier_roles(keymap);
        assert_eq!(
            roles[&42],
            ModRole::Held(mod_bits::SHIFT),
            "Left Shift shifts"
        );
        assert_eq!(roles[&54], ModRole::Held(mod_bits::SHIFT));
        assert_eq!(roles[&29], ModRole::Held(mod_bits::CONTROL));
        assert_eq!(
            roles[&56],
            ModRole::Held(mod_bits::MOD4),
            "Alt is Super, swapped"
        );
        assert_eq!(
            roles[&125],
            ModRole::Held(mod_bits::MOD1),
            "Win is Alt, swapped"
        );
        assert_eq!(roles[&69], ModRole::Locks(mod_bits::MOD2));
        assert_eq!(roles.get(&58), None, "Caps Lock composes here");
        assert_eq!(modifier_roles("not a keymap"), us_modifier_roles());
    }

    /// The keymap of a layout, `us(intl)` naming a variant, from the
    /// system's XKB data.
    fn keymap_text(layout: &str, options: &str) -> String {
        use xkbcommon::xkb;
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let (layout, variant) = layout
            .trim_end_matches(')')
            .split_once('(')
            .unwrap_or((layout, ""));
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "evdev",
            "pc105",
            layout,
            variant,
            Some(options.to_owned()),
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("the system's XKB data has the layout");
        keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
    }

    #[test]
    fn a_controller_s_key_says_what_its_layout_types() {
        use lp_core::input::keyid::{self, mask};
        let ch = |c: char| Some((Some(u32::from(c)), 0));
        let mut gb = KeyMeaning::new(&keymap_text("gb", ""), None).unwrap();
        // The key right of ';' is '\'' on a British layout, '@' with Shift.
        assert_eq!(gb.key(40, true), ch('\''));
        gb.key(40, false);
        assert_eq!(gb.key(42, true), Some((Some(keyid::SHIFT_L), 0)));
        assert_eq!(gb.key(40, true), Some((Some(u32::from('@')), mask::SHIFT)));
        gb.key(40, false);
        assert_eq!(gb.key(3, true), Some((Some(u32::from('"')), mask::SHIFT)));
        gb.key(3, false);
        gb.key(42, false);
        // Control does not change the character: Control+C is 'c'.
        assert_eq!(gb.key(29, true), Some((Some(keyid::CONTROL_L), 0)));
        assert_eq!(
            gb.key(46, true),
            Some((Some(u32::from('c')), mask::CONTROL))
        );
        gb.key(46, false);
        gb.key(29, false);
        assert_eq!(gb.key(28, true), Some((Some(keyid::RETURN), 0)));
        gb.key(28, false);
        // Caps Lock is its key, then the letters it shifts.
        assert_eq!(gb.key(58, true), Some((Some(keyid::CAPS_LOCK), 0)));
        gb.key(58, false);
        assert_eq!(
            gb.key(30, true),
            Some((Some(u32::from('A')), mask::CAPS_LOCK))
        );
        gb.key(30, false);
        // With Alt and the logo key swapped, the Alt position is Super.
        let mut swapped = KeyMeaning::new(&keymap_text("us", "altwin:swap_alt_win"), None).unwrap();
        assert_eq!(swapped.key(56, true), Some((Some(keyid::SUPER_L), 0)));
        // A dead key sends nothing, and the key that completes it the
        // composed character (when the system has the locale's table).
        let mut intl = KeyMeaning::new(&keymap_text("us(intl)", ""), Some("en_US.UTF-8")).unwrap();
        if intl.compose.is_some() {
            assert_eq!(intl.key(40, true), None);
            intl.key(40, false);
            assert_eq!(intl.key(18, true), ch('é'));
            intl.key(18, false);
            assert_eq!(intl.key(18, true), ch('e'));
        }
    }

    #[test]
    fn a_meaning_is_typed_by_the_driven_layout_s_own_key() {
        let shift = mod_bits::SHIFT;
        let mut us = KeyFinder::new(&keymap_text("us", "")).unwrap();
        // '@' is Shift+2 on a US keyboard, on a British one Shift and the key
        // right of ';'.
        assert_eq!(
            us.find(keysym_of(u32::from('@')).unwrap()),
            Some((3, shift))
        );
        let mut gb = KeyFinder::new(&keymap_text("gb", "")).unwrap();
        assert_eq!(
            gb.find(keysym_of(u32::from('@')).unwrap()),
            Some((40, shift))
        );
        assert_eq!(
            gb.find(keysym_of(u32::from('"')).unwrap()),
            Some((3, shift))
        );
        // A letter without Shift, a capital with it, never by Caps Lock.
        assert_eq!(us.find(keysym_of(u32::from('a')).unwrap()), Some((30, 0)));
        assert_eq!(
            us.find(keysym_of(u32::from('A')).unwrap()),
            Some((30, shift))
        );
        // Named keys and modifiers by their keysyms, wherever the layout puts
        // them: with Alt and Win swapped, Alt_L is on the Win key.
        assert_eq!(us.find(keysym_of(keyid::RETURN).unwrap()), Some((28, 0)));
        let mut swapped = KeyFinder::new(&keymap_text("us", "altwin:swap_alt_win")).unwrap();
        assert_eq!(
            swapped.find(keysym_of(keyid::ALT_L).unwrap()).map(|f| f.0),
            Some(125)
        );
        // A character the layout lacks.
        assert_eq!(us.find(keysym_of(u32::from('é')).unwrap()), None);
    }

    #[test]
    fn the_state_reports_held_and_locked_modifiers() {
        let mut m = ModState::new(us_modifier_roles());
        assert_eq!(m.key(30, true), None, "A is no modifier");
        assert_eq!(m.key(42, true), Some((mod_bits::SHIFT, 0)));
        assert_eq!(
            m.key(29, true),
            Some((mod_bits::SHIFT | mod_bits::CONTROL, 0))
        );
        assert_eq!(m.key(42, false), Some((mod_bits::CONTROL, 0)));
        assert_eq!(m.key(29, false), Some((0, 0)));
        // Both Shift keys: one up leaves Shift set.
        m.key(42, true);
        m.key(54, true);
        assert_eq!(m.key(42, false), Some((mod_bits::SHIFT, 0)));
        assert_eq!(m.key(54, false), Some((0, 0)));
        // Caps Lock locks on a press and unlocks on the next.
        assert_eq!(m.key(58, true), Some((0, mod_bits::LOCK)));
        assert_eq!(m.key(58, false), Some((0, mod_bits::LOCK)));
        assert_eq!(m.key(58, true), Some((0, 0)));
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
