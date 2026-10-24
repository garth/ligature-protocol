//! The macOS input backend's arithmetic, kept apart from Quartz so it builds
//! and is tested on every platform (milestone 7, LP-144): the table between
//! macOS virtual key codes and the evdev codes on the wire, the modifier
//! keys (which macOS reports as flag changes, not key presses), the click
//! count a double click needs, and scroll in whole units.

use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Pairs of a macOS virtual key code (`kVK_*`, a physical position on the
/// ANSI layout) and the evdev code of the same position.
const KEYS: &[(u16, u16)] = &[
    (0x00, 30),  // A
    (0x01, 31),  // S
    (0x02, 32),  // D
    (0x03, 33),  // F
    (0x04, 35),  // H
    (0x05, 34),  // G
    (0x06, 44),  // Z
    (0x07, 45),  // X
    (0x08, 46),  // C
    (0x09, 47),  // V
    (0x0A, 86),  // ISO section, the key evdev calls 102ND
    (0x0B, 48),  // B
    (0x0C, 16),  // Q
    (0x0D, 17),  // W
    (0x0E, 18),  // E
    (0x0F, 19),  // R
    (0x10, 21),  // Y
    (0x11, 20),  // T
    (0x12, 2),   // 1
    (0x13, 3),   // 2
    (0x14, 4),   // 3
    (0x15, 5),   // 4
    (0x16, 7),   // 6
    (0x17, 6),   // 5
    (0x18, 13),  // =
    (0x19, 10),  // 9
    (0x1A, 8),   // 7
    (0x1B, 12),  // -
    (0x1C, 9),   // 8
    (0x1D, 11),  // 0
    (0x1E, 27),  // ]
    (0x1F, 24),  // O
    (0x20, 22),  // U
    (0x21, 26),  // [
    (0x22, 23),  // I
    (0x23, 25),  // P
    (0x24, 28),  // Return
    (0x25, 38),  // L
    (0x26, 36),  // J
    (0x27, 40),  // '
    (0x28, 37),  // K
    (0x29, 39),  // ;
    (0x2A, 43),  // backslash
    (0x2B, 51),  // ,
    (0x2C, 53),  // /
    (0x2D, 49),  // N
    (0x2E, 50),  // M
    (0x2F, 52),  // .
    (0x30, 15),  // Tab
    (0x31, 57),  // Space
    (0x32, 41),  // `
    (0x33, 14),  // Delete, evdev's Backspace
    (0x35, 1),   // Escape
    (0x36, 126), // right Command, evdev's right Meta
    (0x37, 125), // Command, evdev's left Meta
    (0x38, 42),  // Shift
    (0x39, 58),  // Caps Lock
    (0x3A, 56),  // Option, evdev's left Alt
    (0x3B, 29),  // Control
    (0x3C, 54),  // right Shift
    (0x3D, 100), // right Option
    (0x3E, 97),  // right Control
    (0x3F, 464), // Function, evdev's KEY_FN
    (0x40, 187), // F17
    (0x41, 83),  // keypad .
    (0x43, 55),  // keypad *
    (0x45, 78),  // keypad +
    (0x47, 69),  // keypad Clear, where Num Lock sits
    (0x48, 115), // volume up
    (0x49, 114), // volume down
    (0x4A, 113), // mute
    (0x4B, 98),  // keypad /
    (0x4C, 96),  // keypad Enter
    (0x4E, 74),  // keypad -
    (0x4F, 188), // F18
    (0x50, 189), // F19
    (0x51, 117), // keypad =
    (0x52, 82),  // keypad 0
    (0x53, 79),  // keypad 1
    (0x54, 80),  // keypad 2
    (0x55, 81),  // keypad 3
    (0x56, 75),  // keypad 4
    (0x57, 76),  // keypad 5
    (0x58, 77),  // keypad 6
    (0x59, 71),  // keypad 7
    (0x5A, 190), // F20
    (0x5B, 72),  // keypad 8
    (0x5C, 73),  // keypad 9
    (0x5D, 124), // JIS Yen
    (0x5E, 89),  // JIS underscore, evdev's RO
    (0x5F, 95),  // JIS keypad comma
    (0x60, 63),  // F5
    (0x61, 64),  // F6
    (0x62, 65),  // F7
    (0x63, 61),  // F3
    (0x64, 66),  // F8
    (0x65, 67),  // F9
    (0x66, 123), // JIS Eisu, HID LANG2, evdev's Hanja
    (0x67, 87),  // F11
    (0x68, 122), // JIS Kana, HID LANG1, evdev's Hangeul
    (0x69, 183), // F13
    (0x6A, 186), // F16
    (0x6B, 184), // F14
    (0x6D, 68),  // F10
    (0x6E, 127), // context menu, evdev's Compose
    (0x6F, 88),  // F12
    (0x71, 185), // F15
    (0x72, 110), // Help, where Insert sits on a PC keyboard
    (0x73, 102), // Home
    (0x74, 104), // Page Up
    (0x75, 111), // forward delete, evdev's Delete
    (0x76, 62),  // F4
    (0x77, 107), // End
    (0x78, 60),  // F2
    (0x79, 109), // Page Down
    (0x7A, 59),  // F1
    (0x7B, 105), // left
    (0x7C, 106), // right
    (0x7D, 108), // down
    (0x7E, 103), // up
];

/// PC keys a Mac has no key for, typed as the keys macOS itself gives them
/// when a PC keyboard is plugged in: one way only.
const PC_ONLY: &[(u16, u16)] = &[
    (99, 0x69),  // Print Screen as F13
    (70, 0x6B),  // Scroll Lock as F14
    (119, 0x71), // Pause as F15
];

/// The evdev code of a macOS virtual key code, `None` for a key with no
/// evdev counterpart (dropped, never guessed).
#[must_use]
pub fn evdev_of(mac: u16) -> Option<u16> {
    KEYS.iter().find(|(m, _)| *m == mac).map(|(_, e)| *e)
}

/// The macOS virtual key code of an evdev code, `None` for a key the Mac
/// cannot type.
#[must_use]
pub fn mac_of(evdev: u16) -> Option<u16> {
    KEYS.iter()
        .find(|(_, e)| *e == evdev)
        .map(|(m, _)| *m)
        .or_else(|| PC_ONLY.iter().find(|(e, _)| *e == evdev).map(|(_, m)| *m))
}

/// The Quartz button number of an evdev button code.
#[must_use]
pub fn mac_button(evdev: u16) -> Option<u32> {
    match evdev {
        272..=276 => Some(u32::from(evdev - 272)),
        _ => None,
    }
}

/// The evdev code of a Quartz button number: left, right, middle, then the
/// side buttons (back and forward).
#[must_use]
pub fn evdev_button(mac: u32) -> Option<u16> {
    u16::try_from(mac).ok().filter(|b| *b <= 4).map(|b| 272 + b)
}

/// The flag bits of `CGEventFlags` the modifiers use.
pub mod flags {
    /// Caps Lock is on.
    pub const ALPHA_SHIFT: u64 = 0x0001_0000;
    /// A Shift key is down.
    pub const SHIFT: u64 = 0x0002_0000;
    /// A Control key is down.
    pub const CONTROL: u64 = 0x0004_0000;
    /// An Option key is down.
    pub const ALTERNATE: u64 = 0x0008_0000;
    /// A Command key is down.
    pub const COMMAND: u64 = 0x0010_0000;
    /// The Function key is down.
    pub const SECONDARY_FN: u64 = 0x0080_0000;
    /// The device-dependent bits that say which side's key is down.
    pub const DEVICE: u64 = 0x0000_207F;
}

/// A modifier key: its macOS code, evdev code, the generic flag and the
/// device bit for its side (zero where there is none).
struct Modifier {
    mac: u16,
    evdev: u16,
    generic: u64,
    device: u64,
}

const MODIFIERS: &[Modifier] = &[
    Modifier {
        mac: 0x38,
        evdev: 42,
        generic: flags::SHIFT,
        device: 0x0002,
    },
    Modifier {
        mac: 0x3C,
        evdev: 54,
        generic: flags::SHIFT,
        device: 0x0004,
    },
    Modifier {
        mac: 0x3B,
        evdev: 29,
        generic: flags::CONTROL,
        device: 0x0001,
    },
    Modifier {
        mac: 0x3E,
        evdev: 97,
        generic: flags::CONTROL,
        device: 0x2000,
    },
    Modifier {
        mac: 0x3A,
        evdev: 56,
        generic: flags::ALTERNATE,
        device: 0x0020,
    },
    Modifier {
        mac: 0x3D,
        evdev: 100,
        generic: flags::ALTERNATE,
        device: 0x0040,
    },
    Modifier {
        mac: 0x37,
        evdev: 125,
        generic: flags::COMMAND,
        device: 0x0008,
    },
    Modifier {
        mac: 0x36,
        evdev: 126,
        generic: flags::COMMAND,
        device: 0x0010,
    },
    Modifier {
        mac: 0x3F,
        evdev: 464,
        generic: flags::SECONDARY_FN,
        device: 0,
    },
];

const CAPS_LOCK_MAC: u16 = 0x39;
const CAPS_LOCK_EVDEV: u16 = 58;

/// Whether an evdev code is a modifier, which macOS posts as a flag change.
#[must_use]
pub fn is_modifier(evdev: u16) -> bool {
    evdev == CAPS_LOCK_EVDEV || MODIFIERS.iter().any(|m| m.evdev == evdev)
}

/// The capture side: turns macOS's flag-change events into evdev presses
/// and releases.
#[derive(Debug, Default)]
pub struct ModifierCapture {
    held: HashSet<u16>,
}

impl ModifierCapture {
    /// A flag change for the key `mac` with the event's `flags`: the evdev
    /// key events it stands for. Caps Lock reports only its new state, so it
    /// is a press and a release each time; the others are down while their
    /// side's device bit is set, or, when a source sets no device bits,
    /// toggle.
    pub fn changed(&mut self, mac: u16, event_flags: u64) -> Vec<(u16, bool)> {
        if mac == CAPS_LOCK_MAC {
            return vec![(CAPS_LOCK_EVDEV, true), (CAPS_LOCK_EVDEV, false)];
        }
        let Some(m) = MODIFIERS.iter().find(|m| m.mac == mac) else {
            return Vec::new();
        };
        let was = self.held.contains(&mac);
        let pressed = if m.device != 0 && event_flags & flags::DEVICE != 0 {
            event_flags & m.device != 0
        } else if m.device == 0 {
            event_flags & m.generic != 0
        } else {
            !was
        };
        if pressed == was {
            return Vec::new();
        }
        if pressed {
            self.held.insert(mac);
        } else {
            self.held.remove(&mac);
        }
        vec![(m.evdev, pressed)]
    }

    /// Forgets what is held, when a capture starts or ends.
    pub fn clear(&mut self) {
        self.held.clear();
    }
}

/// The injection side: which modifiers are down, as the flags every posted
/// event carries.
#[derive(Debug, Default)]
pub struct ModifierInject {
    held: HashSet<u16>,
    caps: bool,
}

impl ModifierInject {
    /// A key went down or up; returns whether it was a modifier.
    pub fn key(&mut self, evdev: u16, pressed: bool) -> bool {
        if evdev == CAPS_LOCK_EVDEV {
            if pressed {
                self.caps = !self.caps;
            }
            return true;
        }
        if !MODIFIERS.iter().any(|m| m.evdev == evdev) {
            return false;
        }
        if pressed {
            self.held.insert(evdev);
        } else {
            self.held.remove(&evdev);
        }
        true
    }

    /// The flags for the next posted event.
    #[must_use]
    pub fn flags(&self) -> u64 {
        let mut out = if self.caps { flags::ALPHA_SHIFT } else { 0 };
        for m in MODIFIERS.iter().filter(|m| self.held.contains(&m.evdev)) {
            out |= m.generic | m.device;
        }
        out
    }

    /// The modifiers held, as evdev codes in ascending order.
    #[must_use]
    pub fn held(&self) -> Vec<u16> {
        let mut held: Vec<u16> = self.held.iter().copied().collect();
        held.sort_unstable();
        held
    }

    /// Lets go of every modifier, keeping Caps Lock's state, which is a
    /// toggle and not held.
    pub fn clear(&mut self) {
        self.held.clear();
    }
}

/// Counts clicks for the click state Quartz needs on a button event: a press
/// of the same button soon after the last, close to it, is a double click.
#[derive(Debug)]
pub struct ClickCounter {
    interval: Duration,
    last: Option<(u16, Instant, (f64, f64))>,
    count: i64,
}

/// How far, in points, a second click may land from the first.
const CLICK_SLOP: f64 = 4.0;

impl ClickCounter {
    /// A counter with the system's double-click interval.
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
            count: 0,
        }
    }

    /// `button` was pressed at `at` at `now`: the click count for the press
    /// and for its release.
    pub fn press(&mut self, button: u16, at: (f64, f64), now: Instant) -> i64 {
        let again = self.last.is_some_and(|(b, t, p)| {
            b == button
                && now.duration_since(t) <= self.interval
                && (p.0 - at.0).abs() <= CLICK_SLOP
                && (p.1 - at.1).abs() <= CLICK_SLOP
        });
        self.count = if again { self.count + 1 } else { 1 };
        self.last = Some((button, now, at));
        self.count
    }

    /// The count to put on a release: the press's.
    #[must_use]
    pub fn current(&self) -> i64 {
        self.count.max(1)
    }

    /// The pointer moved: a click after a movement starts again.
    pub fn moved(&mut self, at: (f64, f64)) {
        if let Some((_, _, p)) = self.last
            && ((p.0 - at.0).abs() > CLICK_SLOP || (p.1 - at.1).abs() > CLICK_SLOP)
        {
            self.last = None;
        }
    }
}

/// Scroll in whole units: Quartz takes integer lines or pixels, the wire
/// carries fractions (a high-resolution wheel sends part steps), so the
/// remainder waits for the next event.
#[derive(Debug, Default)]
pub struct ScrollUnits {
    x: f64,
    y: f64,
}

impl ScrollUnits {
    /// Adds `dx`, `dy` in the wire's sense (positive is right and down) and
    /// returns the whole units to post in Quartz's (positive is left and
    /// up): vertical first, as `CGEventCreateScrollWheelEvent2` orders them.
    pub fn take(&mut self, dx: f64, dy: f64) -> (i32, i32) {
        self.x += dx;
        self.y += dy;
        let wx = self.x.trunc();
        let wy = self.y.trunc();
        self.x -= wx;
        self.y -= wy;
        (-(wy as i32), -(wx as i32))
    }

    /// Forgets the remainder, when the units change.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// A captured Quartz scroll in the wire's sense: `continuous` (a trackpad or
/// a smooth mouse) reports points, a wheel reports lines as discrete steps.
/// Quartz's axes are positive up and left, the wire's down and right.
#[must_use]
pub fn wire_scroll(
    continuous: bool,
    lines: (f64, f64),
    points: (f64, f64),
) -> lp_core::input::InputEvent {
    let (vertical, horizontal) = if continuous { points } else { lines };
    lp_core::input::InputEvent::Scroll {
        dx: -horizontal,
        dy: -vertical,
        discrete: !continuous,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_maps_both_ways_and_no_code_is_used_twice() {
        let mut macs = HashSet::new();
        let mut evdevs = HashSet::new();
        for (m, e) in KEYS {
            assert!(macs.insert(*m), "mac code {m:#x} twice");
            assert!(evdevs.insert(*e), "evdev code {e} twice");
            assert_eq!(evdev_of(*m), Some(*e));
            assert_eq!(mac_of(*e), Some(*m));
        }
        for (e, _) in PC_ONLY {
            assert!(!evdevs.contains(e), "a PC-only key is also in the table");
        }
        // The letters of "ligature" and the release chord's keys.
        let typed: Vec<u16> = [0x25, 0x22, 0x05, 0x00, 0x11, 0x20, 0x0F, 0x0E]
            .iter()
            .map(|m| evdev_of(*m).unwrap())
            .collect();
        assert_eq!(typed, vec![38, 23, 34, 30, 20, 22, 19, 18]);
        assert_eq!(mac_of(lp_core::input::KEY_LEFTCTRL), Some(0x3B));
        assert_eq!(mac_of(lp_core::input::KEY_RIGHTCTRL), Some(0x3E));
        assert_eq!(mac_of(99), Some(0x69), "Print Screen types F13");
        assert_eq!(evdev_of(0x34), None, "an unused code is dropped");
        assert_eq!(mac_of(240), None);
    }

    #[test]
    fn buttons_map_left_right_middle_and_the_side_buttons() {
        assert_eq!(mac_button(272), Some(0));
        assert_eq!(mac_button(273), Some(1));
        assert_eq!(mac_button(274), Some(2));
        assert_eq!(mac_button(276), Some(4));
        assert_eq!(mac_button(277), None);
        assert_eq!(evdev_button(1), Some(273));
        assert_eq!(evdev_button(5), None);
    }

    #[test]
    fn flag_changes_become_presses_and_releases_by_side() {
        let mut m = ModifierCapture::default();
        // Left Shift down: the generic and its device bit.
        assert_eq!(m.changed(0x38, flags::SHIFT | 0x02), vec![(42, true)]);
        // Right Shift down too.
        assert_eq!(m.changed(0x3C, flags::SHIFT | 0x06), vec![(54, true)]);
        // Left up while right stays: the generic flag is still set.
        assert_eq!(m.changed(0x38, flags::SHIFT | 0x04), vec![(42, false)]);
        assert_eq!(m.changed(0x3C, 0), vec![(54, false)]);
        // A source without device bits toggles.
        assert_eq!(m.changed(0x3B, flags::CONTROL), vec![(29, true)]);
        assert_eq!(m.changed(0x3B, 0), vec![(29, false)]);
        // Fn has no device bit and follows its generic flag.
        assert_eq!(m.changed(0x3F, flags::SECONDARY_FN), vec![(464, true)]);
        assert_eq!(m.changed(0x3F, flags::SECONDARY_FN), vec![]);
        assert_eq!(m.changed(0x3F, 0), vec![(464, false)]);
        // Caps Lock is a press and a release each time it changes.
        assert_eq!(
            m.changed(0x39, flags::ALPHA_SHIFT),
            vec![(58, true), (58, false)]
        );
        assert_eq!(m.changed(0x00, 0), vec![], "not a modifier");
    }

    #[test]
    fn injected_events_carry_the_held_modifiers_flags() {
        let mut m = ModifierInject::default();
        assert!(m.key(29, true));
        assert!(m.key(42, true));
        assert_eq!(m.flags(), flags::CONTROL | 0x01 | flags::SHIFT | 0x02);
        assert!(!m.key(30, true), "A is not a modifier");
        assert_eq!(m.held(), vec![29, 42]);
        assert!(m.key(42, false));
        assert_eq!(m.flags(), flags::CONTROL | 0x01);
        assert!(m.key(58, true));
        assert!(m.key(58, false));
        m.clear();
        assert_eq!(m.flags(), flags::ALPHA_SHIFT, "Caps Lock stays on");
        assert!(is_modifier(125) && is_modifier(58) && !is_modifier(30));
    }

    #[test]
    fn a_second_press_close_and_soon_is_a_double_click() {
        let mut c = ClickCounter::new(Duration::from_millis(500));
        let t = Instant::now();
        assert_eq!(c.press(272, (10.0, 10.0), t), 1);
        assert_eq!(c.current(), 1);
        assert_eq!(
            c.press(272, (11.0, 10.0), t + Duration::from_millis(200)),
            2
        );
        assert_eq!(
            c.press(272, (11.0, 10.0), t + Duration::from_millis(350)),
            3
        );
        // Too late, another button, or moved away: one again.
        assert_eq!(c.press(272, (11.0, 10.0), t + Duration::from_secs(2)), 1);
        assert_eq!(
            c.press(273, (11.0, 10.0), t + Duration::from_millis(2100)),
            1
        );
        c.moved((50.0, 50.0));
        assert_eq!(
            c.press(273, (50.0, 50.0), t + Duration::from_millis(2200)),
            1
        );
    }

    #[test]
    fn scroll_keeps_the_fractions_and_flips_to_quartz_s_sense() {
        let mut s = ScrollUnits::default();
        assert_eq!(s.take(0.0, 0.5), (0, 0));
        assert_eq!(s.take(0.0, 0.5), (-1, 0), "two half steps down are one");
        assert_eq!(s.take(2.0, -3.0), (3, -2));
        let wire = wire_scroll(false, (1.0, 0.0), (10.0, 0.0));
        assert_eq!(
            wire,
            lp_core::input::InputEvent::Scroll {
                dx: -0.0,
                dy: -1.0,
                discrete: true
            }
        );
        let wire = wire_scroll(true, (1.0, 0.0), (12.0, -4.0));
        assert_eq!(
            wire,
            lp_core::input::InputEvent::Scroll {
                dx: 4.0,
                dy: -12.0,
                discrete: false
            }
        );
    }
}
