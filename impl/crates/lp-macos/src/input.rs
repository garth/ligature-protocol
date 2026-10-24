//! Quartz input (milestone 7, LP-144): an event tap that sees, and may
//! swallow, the user's keyboard and mouse; an event poster that types and
//! points as if a device were attached; the cursor's position, warping,
//! freezing and hiding; the displays; and the two permissions both need.
//!
//! The tap runs on a thread of its own with its own run loop. Its handler is
//! called for every event and answers whether to swallow it. Events this
//! crate posted carry a tag in their source's user data, and the tap passes
//! them without calling the handler, so a Mac being driven does not mistake
//! the pointer it is driven by for the user's.

use std::ffi::{c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use objc2_app_kit::NSEvent;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFMachPortRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CGEventRef = *mut c_void;
type CGEventSourceRef = *mut c_void;
type CGEventTapProxy = *mut c_void;
type TapCallback = extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayHideCursor(display: u32) -> i32;
    fn CGDisplayShowCursor(display: u32) -> i32;
    fn CGWarpMouseCursorPosition(point: CGPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: u32) -> i32;
    fn CGEventCreate(source: CGEventSourceRef) -> CGEventRef;
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventGetDoubleValueField(event: CGEventRef, field: u32) -> f64;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventSetType(event: CGEventRef, kind: u32);
    fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        kind: u32,
        at: CGPoint,
        button: u32,
    ) -> CGEventRef;
    fn CGEventCreateKeyboardEvent(source: CGEventSourceRef, code: u16, down: bool) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent2(
        source: CGEventSourceRef,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;
    fn CGEventKeyboardSetUnicodeString(event: CGEventRef, length: usize, string: *const u16);
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGEventSourceCreate(state: i32) -> CGEventSourceRef;
    fn CGEventSourceSetUserData(source: CGEventSourceRef, data: i64);
    fn CGEventSourceSetLocalEventsSuppressionInterval(source: CGEventSourceRef, seconds: f64);
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        mask: u64,
        callback: TapCallback,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: CFStringRef;
    static kCFBooleanTrue: CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFMachPortCreateRunLoopSource(
        allocator: CFTypeRef,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;
    fn CFMachPortInvalidate(port: CFMachPortRef);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(run_loop: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRun();
    fn CFRunLoopStop(run_loop: CFRunLoopRef);
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        string: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
}

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

// Event types.
const LEFT_MOUSE_DOWN: u32 = 1;
const LEFT_MOUSE_UP: u32 = 2;
const RIGHT_MOUSE_DOWN: u32 = 3;
const RIGHT_MOUSE_UP: u32 = 4;
const MOUSE_MOVED: u32 = 5;
const LEFT_MOUSE_DRAGGED: u32 = 6;
const RIGHT_MOUSE_DRAGGED: u32 = 7;
const KEY_DOWN: u32 = 10;
const KEY_UP: u32 = 11;
const FLAGS_CHANGED: u32 = 12;
const SCROLL_WHEEL: u32 = 22;
const OTHER_MOUSE_DOWN: u32 = 25;
const OTHER_MOUSE_UP: u32 = 26;
const OTHER_MOUSE_DRAGGED: u32 = 27;
const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

// Event fields.
const MOUSE_CLICK_STATE: u32 = 1;
const MOUSE_BUTTON_NUMBER: u32 = 3;
const MOUSE_DELTA_X: u32 = 4;
const MOUSE_DELTA_Y: u32 = 5;
const KEYBOARD_AUTOREPEAT: u32 = 8;
const KEYBOARD_KEYCODE: u32 = 9;
const SCROLL_DELTA_AXIS_1: u32 = 11;
const SCROLL_DELTA_AXIS_2: u32 = 12;
const SCROLL_IS_CONTINUOUS: u32 = 88;
const SCROLL_POINT_DELTA_AXIS_1: u32 = 96;
const SCROLL_POINT_DELTA_AXIS_2: u32 = 97;
const SOURCE_USER_DATA: u32 = 42;

// Tap placement and posting.
const HID_EVENT_TAP: u32 = 0;
const SESSION_EVENT_TAP: u32 = 1;
const HEAD_INSERT: u32 = 0;
const TAP_DEFAULT: u32 = 0;
const COMBINED_SESSION_STATE: i32 = 0;
const SCROLL_UNIT_PIXEL: u32 = 0;
const SCROLL_UNIT_LINE: u32 = 1;
const UTF8: u32 = 0x0800_0100;
/// `RTLD_DEFAULT` on macOS.
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

/// The user data the poster's events carry, "LIGATURE" in ASCII.
const INJECTED_TAG: i64 = 0x4C49_4741_5455_5245;
/// Quartz's longest string on one keyboard event, in UTF-16 units.
const TEXT_CHUNK: usize = 20;

/// Whether the process may watch the keyboard (Input Monitoring).
#[must_use]
pub fn listen_access() -> bool {
    // SAFETY: a plain query with no arguments.
    unsafe { CGPreflightListenEventAccess() }
}

/// Whether the process may post events and filter them (Accessibility).
#[must_use]
pub fn post_access() -> bool {
    // SAFETY: a plain query with no arguments.
    unsafe { CGPreflightPostEventAccess() }
}

/// Asks for whichever of the two permissions is missing: macOS shows its
/// prompt, which leads to System Settings, once per permission.
pub fn request_access() {
    // SAFETY: plain requests with no arguments.
    unsafe {
        if !CGPreflightListenEventAccess() {
            CGRequestListenEventAccess();
        }
        if !CGPreflightPostEventAccess() {
            CGRequestPostEventAccess();
        }
    }
}

/// A display's rectangle in the global space, in points, the origin at the
/// main display's top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayRect {
    /// Left.
    pub x: f64,
    /// Top.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// The active displays.
#[must_use]
pub fn displays() -> Vec<DisplayRect> {
    display_list().into_iter().map(|(_, r)| r).collect()
}

/// The active displays with their `CGDirectDisplayID`s, and the main one's.
pub(crate) fn display_list() -> Vec<(u32, DisplayRect)> {
    let mut ids = [0u32; 32];
    let mut count = 0u32;
    // SAFETY: the buffer holds 32 ids, the count it is given.
    let err = unsafe { CGGetActiveDisplayList(32, ids.as_mut_ptr(), &mut count) };
    if err != 0 {
        return Vec::new();
    }
    ids[..count.min(32) as usize]
        .iter()
        .map(|id| {
            // SAFETY: an id the system just listed.
            let r = unsafe { CGDisplayBounds(*id) };
            (
                *id,
                DisplayRect {
                    x: r.origin.x,
                    y: r.origin.y,
                    width: r.size.width,
                    height: r.size.height,
                },
            )
        })
        .collect()
}

/// The display with the menu bar.
pub(crate) fn main_display() -> u32 {
    // SAFETY: a plain query with no arguments.
    unsafe { CGMainDisplayID() }
}

/// The cursor's position in the global space.
#[must_use]
pub fn cursor() -> (f64, f64) {
    // SAFETY: an event made without a source only to read the location, and
    // released after.
    unsafe {
        let e = CGEventCreate(std::ptr::null_mut());
        if e.is_null() {
            return (0.0, 0.0);
        }
        let p = CGEventGetLocation(e);
        CFRelease(e);
        (p.x, p.y)
    }
}

/// Moves the cursor without making an event.
pub fn warp(at: (f64, f64)) {
    // SAFETY: a plain call with a point.
    unsafe {
        CGWarpMouseCursorPosition(CGPoint { x: at.0, y: at.1 });
    }
}

/// Freezes and hides the cursor while the keyboard and mouse drive another
/// device (`true`), or lets it move and shows it again. The mouse still
/// reports its motion to the tap while frozen.
///
/// Hiding the cursor from a process that is not the active application
/// needs the window server's `SetsCursorInBackground` connection property,
/// which is not public API; it is looked up at run time and skipped if
/// absent, in which case the cursor stays visible but still.
pub fn hold_cursor(held: bool) {
    // SAFETY: plain calls; the hide and show are balanced by the caller,
    // which calls this alternately.
    unsafe {
        CGAssociateMouseAndMouseCursorPosition(u32::from(!held));
        if held {
            allow_background_cursor();
            CGDisplayHideCursor(CGMainDisplayID());
        } else {
            CGDisplayShowCursor(CGMainDisplayID());
        }
    }
}

fn allow_background_cursor() {
    type DefaultConnection = unsafe extern "C" fn() -> i32;
    type SetProperty = unsafe extern "C" fn(i32, i32, CFStringRef, CFTypeRef) -> i32;
    // SAFETY: the symbols are looked up by name and called with the
    // signatures they have had since Mac OS X 10.4; a missing one is skipped.
    unsafe {
        let conn = dlsym(RTLD_DEFAULT, c"_CGSDefaultConnection".as_ptr());
        let set = dlsym(RTLD_DEFAULT, c"CGSSetConnectionProperty".as_ptr());
        if conn.is_null() || set.is_null() {
            return;
        }
        let conn: DefaultConnection = std::mem::transmute(conn);
        let set: SetProperty = std::mem::transmute(set);
        let key =
            CFStringCreateWithCString(std::ptr::null(), c"SetsCursorInBackground".as_ptr(), UTF8);
        if key.is_null() {
            return;
        }
        let cid = conn();
        set(cid, cid, key, kCFBooleanTrue);
        CFRelease(key);
    }
}

/// The user's double-click interval.
#[must_use]
pub fn double_click_interval() -> Duration {
    Duration::from_secs_f64(NSEvent::doubleClickInterval().clamp(0.1, 5.0))
}

/// One event the tap saw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TapEvent {
    /// The mouse moved, with or without a button down: where the cursor is
    /// and how far the mouse moved, in points, which it reports even while
    /// the cursor is frozen.
    Moved {
        /// The cursor's position.
        at: (f64, f64),
        /// Horizontal motion.
        dx: f64,
        /// Vertical motion, positive downwards.
        dy: f64,
    },
    /// A mouse button: Quartz's number, 0 left, 1 right, 2 middle.
    Button {
        /// The button number.
        button: u32,
        /// Down or up.
        pressed: bool,
    },
    /// Scrolling, in Quartz's sense: positive is up and left.
    Scroll {
        /// From a trackpad or a smooth mouse, in points, rather than a wheel.
        continuous: bool,
        /// Vertical and horizontal, in lines.
        lines: (f64, f64),
        /// Vertical and horizontal, in points.
        points: (f64, f64),
    },
    /// A key that is not a modifier.
    Key {
        /// The virtual key code.
        code: u16,
        /// Down or up.
        pressed: bool,
        /// A repeat the system made while the key is held.
        repeat: bool,
    },
    /// A modifier key changed: its virtual key code and the flags after.
    Flags {
        /// The virtual key code.
        code: u16,
        /// `CGEventFlags`.
        flags: u64,
    },
    /// The system had disabled the tap (it was too slow, or secure input
    /// began); it has been enabled again.
    Disabled,
}

type Handler = Box<dyn FnMut(TapEvent) -> bool + Send>;

struct TapContext {
    handler: Handler,
    port: CFMachPortRef,
}

/// A raw pointer handed between threads: the run loop to stop.
struct SendPtr(*mut c_void);
// SAFETY: a CFRunLoop may be stopped from any thread.
unsafe impl Send for SendPtr {}

/// A running event tap; dropping it stops the tap and its thread.
pub struct Tap {
    run_loop: SendPtr,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Tap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tap")
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        // SAFETY: the run loop lives as long as its thread, joined below.
        unsafe { CFRunLoopStop(self.run_loop.0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Starts a tap on the session's events, ahead of every application. The
/// handler sees each event and returns `true` to swallow it.
///
/// # Errors
///
/// Returns a message when the tap cannot be made, which is what happens
/// without the Accessibility and Input Monitoring permissions.
pub fn tap(handler: impl FnMut(TapEvent) -> bool + Send + 'static) -> Result<Tap, String> {
    let (tx, rx) = mpsc::channel::<Result<SendPtr, String>>();
    let thread = std::thread::Builder::new()
        .name("ligature-event-tap".to_owned())
        .spawn(move || run_tap(Box::new(handler), &tx))
        .map_err(|e| format!("cannot start the event tap thread: {e}"))?;
    match rx.recv() {
        Ok(Ok(run_loop)) => Ok(Tap {
            run_loop,
            thread: Some(thread),
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err("the event tap thread ended".to_owned()),
    }
}

fn run_tap(handler: Handler, tx: &mpsc::Sender<Result<SendPtr, String>>) {
    let mask: u64 = [
        LEFT_MOUSE_DOWN,
        LEFT_MOUSE_UP,
        RIGHT_MOUSE_DOWN,
        RIGHT_MOUSE_UP,
        MOUSE_MOVED,
        LEFT_MOUSE_DRAGGED,
        RIGHT_MOUSE_DRAGGED,
        KEY_DOWN,
        KEY_UP,
        FLAGS_CHANGED,
        SCROLL_WHEEL,
        OTHER_MOUSE_DOWN,
        OTHER_MOUSE_UP,
        OTHER_MOUSE_DRAGGED,
    ]
    .iter()
    .fold(0, |m, t| m | (1u64 << t));
    let context = Box::into_raw(Box::new(TapContext {
        handler,
        port: std::ptr::null_mut(),
    }));
    // SAFETY: the context outlives the tap: it is freed below, after the run
    // loop has returned and the tap is invalidated. The run loop source and
    // the port are released here too.
    unsafe {
        let port = CGEventTapCreate(
            SESSION_EVENT_TAP,
            HEAD_INSERT,
            TAP_DEFAULT,
            mask,
            tap_callback,
            context.cast(),
        );
        if port.is_null() {
            drop(Box::from_raw(context));
            let _ = tx.send(Err(
                "the event tap was refused: Accessibility and Input Monitoring are needed"
                    .to_owned(),
            ));
            return;
        }
        (*context).port = port;
        let source = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
        CGEventTapEnable(port, true);
        let _ = tx.send(Ok(SendPtr(run_loop)));
        CFRunLoopRun();
        CGEventTapEnable(port, false);
        CFMachPortInvalidate(port);
        CFRelease(source);
        CFRelease(port);
        drop(Box::from_raw(context));
    }
}

extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    kind: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    // SAFETY: `user_info` is the context `run_tap` made, alive while the tap
    // runs, and only this thread touches it; `event` is the system's event
    // for the duration of the call.
    unsafe {
        let context = &mut *user_info.cast::<TapContext>();
        if kind == TAP_DISABLED_BY_TIMEOUT || kind == TAP_DISABLED_BY_USER_INPUT {
            CGEventTapEnable(context.port, true);
            let _ = catch_unwind(AssertUnwindSafe(|| (context.handler)(TapEvent::Disabled)));
            return event;
        }
        if event.is_null() || CGEventGetIntegerValueField(event, SOURCE_USER_DATA) == INJECTED_TAG {
            return event;
        }
        let Some(decoded) = decode(kind, event) else {
            return event;
        };
        match catch_unwind(AssertUnwindSafe(|| (context.handler)(decoded))) {
            Ok(true) => std::ptr::null_mut(),
            _ => event,
        }
    }
}

/// Reads one event into a [`TapEvent`].
///
/// # Safety
///
/// `event` is a live event.
unsafe fn decode(kind: u32, event: CGEventRef) -> Option<TapEvent> {
    // SAFETY: the caller's.
    unsafe {
        let int = |field| CGEventGetIntegerValueField(event, field);
        let double = |field| CGEventGetDoubleValueField(event, field);
        Some(match kind {
            MOUSE_MOVED | LEFT_MOUSE_DRAGGED | RIGHT_MOUSE_DRAGGED | OTHER_MOUSE_DRAGGED => {
                let p = CGEventGetLocation(event);
                TapEvent::Moved {
                    at: (p.x, p.y),
                    dx: double(MOUSE_DELTA_X),
                    dy: double(MOUSE_DELTA_Y),
                }
            }
            LEFT_MOUSE_DOWN | RIGHT_MOUSE_DOWN | OTHER_MOUSE_DOWN => TapEvent::Button {
                button: u32::try_from(int(MOUSE_BUTTON_NUMBER)).unwrap_or(0),
                pressed: true,
            },
            LEFT_MOUSE_UP | RIGHT_MOUSE_UP | OTHER_MOUSE_UP => TapEvent::Button {
                button: u32::try_from(int(MOUSE_BUTTON_NUMBER)).unwrap_or(0),
                pressed: false,
            },
            SCROLL_WHEEL => TapEvent::Scroll {
                continuous: int(SCROLL_IS_CONTINUOUS) != 0,
                lines: (
                    int(SCROLL_DELTA_AXIS_1) as f64,
                    int(SCROLL_DELTA_AXIS_2) as f64,
                ),
                points: (
                    double(SCROLL_POINT_DELTA_AXIS_1),
                    double(SCROLL_POINT_DELTA_AXIS_2),
                ),
            },
            KEY_DOWN | KEY_UP => TapEvent::Key {
                code: u16::try_from(int(KEYBOARD_KEYCODE)).unwrap_or(u16::MAX),
                pressed: kind == KEY_DOWN,
                repeat: int(KEYBOARD_AUTOREPEAT) != 0,
            },
            FLAGS_CHANGED => TapEvent::Flags {
                code: u16::try_from(int(KEYBOARD_KEYCODE)).unwrap_or(u16::MAX),
                flags: CGEventGetFlags(event),
            },
            _ => return None,
        })
    }
}

/// What a posted mouse event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mouse {
    /// The pointer moved with no button down.
    Moved,
    /// The pointer moved with this button down.
    Dragged(u32),
    /// This button went down.
    Down(u32),
    /// This button went up.
    Up(u32),
}

/// Posts events as if from a keyboard and mouse attached to this Mac. Its
/// events carry the tag the tap passes over.
pub struct Poster {
    source: CGEventSourceRef,
}

// SAFETY: an event source is a CoreFoundation object the system allows on
// any thread; the poster only reads it after creation.
unsafe impl Send for Poster {}
// SAFETY: as above.
unsafe impl Sync for Poster {}

impl std::fmt::Debug for Poster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Poster")
    }
}

impl Drop for Poster {
    fn drop(&mut self) {
        // SAFETY: the source this poster created and still owns.
        unsafe { CFRelease(self.source) };
    }
}

impl Poster {
    /// A poster, or `None` if the system gives no event source.
    #[must_use]
    pub fn new() -> Option<Self> {
        // SAFETY: plain calls on the source made here.
        unsafe {
            let source = CGEventSourceCreate(COMBINED_SESSION_STATE);
            if source.is_null() {
                return None;
            }
            CGEventSourceSetUserData(source, INJECTED_TAG);
            CGEventSourceSetLocalEventsSuppressionInterval(source, 0.0);
            Some(Self { source })
        }
    }

    /// Posts and releases `event`, if one was made.
    ///
    /// # Safety
    ///
    /// `event` is null or an event this poster created.
    unsafe fn post(event: CGEventRef) {
        if event.is_null() {
            return;
        }
        // SAFETY: the caller's.
        unsafe {
            CGEventPost(HID_EVENT_TAP, event);
            CFRelease(event);
        }
    }

    /// A mouse event at `at`, with the motion that brought it there, the
    /// click count for a button event, and the modifier flags.
    pub fn mouse(&self, kind: Mouse, at: (f64, f64), delta: (f64, f64), clicks: i64, flags: u64) {
        let (event_type, button) = match kind {
            Mouse::Moved => (MOUSE_MOVED, 0),
            Mouse::Dragged(0) => (LEFT_MOUSE_DRAGGED, 0),
            Mouse::Dragged(1) => (RIGHT_MOUSE_DRAGGED, 1),
            Mouse::Dragged(b) => (OTHER_MOUSE_DRAGGED, b),
            Mouse::Down(0) => (LEFT_MOUSE_DOWN, 0),
            Mouse::Down(1) => (RIGHT_MOUSE_DOWN, 1),
            Mouse::Down(b) => (OTHER_MOUSE_DOWN, b),
            Mouse::Up(0) => (LEFT_MOUSE_UP, 0),
            Mouse::Up(1) => (RIGHT_MOUSE_UP, 1),
            Mouse::Up(b) => (OTHER_MOUSE_UP, b),
        };
        // SAFETY: an event made from this poster's source, posted and
        // released by `post`.
        unsafe {
            let e = CGEventCreateMouseEvent(
                self.source,
                event_type,
                CGPoint { x: at.0, y: at.1 },
                button.min(2),
            );
            if e.is_null() {
                return;
            }
            CGEventSetIntegerValueField(e, MOUSE_BUTTON_NUMBER, i64::from(button));
            match kind {
                Mouse::Moved | Mouse::Dragged(_) => {
                    CGEventSetIntegerValueField(e, MOUSE_DELTA_X, delta.0.round() as i64);
                    CGEventSetIntegerValueField(e, MOUSE_DELTA_Y, delta.1.round() as i64);
                }
                Mouse::Down(_) | Mouse::Up(_) => {
                    CGEventSetIntegerValueField(e, MOUSE_CLICK_STATE, clicks.max(1));
                }
            }
            CGEventSetFlags(e, flags);
            Self::post(e);
        }
    }

    /// A scroll in Quartz's sense (positive up and left), in lines or in
    /// pixels.
    pub fn scroll(&self, lines: bool, vertical: i32, horizontal: i32, flags: u64) {
        if vertical == 0 && horizontal == 0 {
            return;
        }
        let units = if lines {
            SCROLL_UNIT_LINE
        } else {
            SCROLL_UNIT_PIXEL
        };
        // SAFETY: as in `mouse`.
        unsafe {
            let e = CGEventCreateScrollWheelEvent2(self.source, units, 2, vertical, horizontal, 0);
            if e.is_null() {
                return;
            }
            CGEventSetFlags(e, flags);
            Self::post(e);
        }
    }

    /// A key, by virtual key code; a modifier is posted as the flag change
    /// macOS makes for one.
    pub fn key(&self, code: u16, pressed: bool, modifier: bool, flags: u64) {
        // SAFETY: as in `mouse`.
        unsafe {
            let e = CGEventCreateKeyboardEvent(self.source, code, pressed);
            if e.is_null() {
                return;
            }
            if modifier {
                CGEventSetType(e, FLAGS_CHANGED);
            }
            CGEventSetFlags(e, flags);
            Self::post(e);
        }
    }

    /// Types `text` as characters, whatever the keyboard layout, as key
    /// events with the string attached, a few characters at a time.
    pub fn text(&self, text: &str) {
        let mut chunk: Vec<u16> = Vec::with_capacity(TEXT_CHUNK);
        let flush = |chunk: &mut Vec<u16>| {
            if chunk.is_empty() {
                return;
            }
            for down in [true, false] {
                // SAFETY: as in `mouse`; the string outlives the call that
                // copies it into the event.
                unsafe {
                    let e = CGEventCreateKeyboardEvent(self.source, 0, down);
                    if e.is_null() {
                        continue;
                    }
                    CGEventKeyboardSetUnicodeString(e, chunk.len(), chunk.as_ptr());
                    CGEventSetFlags(e, 0);
                    Self::post(e);
                }
            }
            chunk.clear();
        };
        let mut units = [0u16; 2];
        for c in text.chars() {
            let encoded = c.encode_utf16(&mut units);
            if chunk.len() + encoded.len() > TEXT_CHUNK {
                flush(&mut chunk);
            }
            chunk.extend_from_slice(encoded);
        }
        flush(&mut chunk);
    }
}
