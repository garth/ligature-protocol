//! Synergy's key IDs and modifier mask, which a `key` event carries as `id`
//! and `mods` (ADR 0043).
//!
//! A key ID is what a key means on the controller's layout: a character's
//! Unicode scalar value, or for a named key a value in the private-use block
//! U+E000 to U+EFFF, which for the keys of the X11 keysym range 0xFF00 to
//! 0xFFFF is the keysym less 0x1000. The values are Synergy's, as Input Leap
//! keeps them in `src/lib/inputleap/key_types.h`.

/// A key ID.
pub type KeyId = u32;

/// The modifier mask bits.
pub mod mask {
    /// Shift.
    pub const SHIFT: u32 = 0x0001;
    /// Control.
    pub const CONTROL: u32 = 0x0002;
    /// Alt (a Mac's Option).
    pub const ALT: u32 = 0x0004;
    /// Meta.
    pub const META: u32 = 0x0008;
    /// Super (a Mac's Command).
    pub const SUPER: u32 = 0x0010;
    /// AltGr, the third level.
    pub const ALT_GR: u32 = 0x0020;
    /// Caps Lock is on.
    pub const CAPS_LOCK: u32 = 0x1000;
    /// Num Lock is on.
    pub const NUM_LOCK: u32 = 0x2000;
    /// Scroll Lock is on.
    pub const SCROLL_LOCK: u32 = 0x4000;
    /// The modifiers that make a key a shortcut rather than a character:
    /// with any held, the driven device presses the key that produces the ID
    /// with the user's modifiers, not the exact character. AltGr is not
    /// among them, unlike in Synergy: it chooses a character's level, as
    /// Shift does, so a character typed with it is typed exactly.
    pub const COMMAND: u32 = CONTROL | ALT | META | SUPER;
}

/// Backspace.
pub const BACKSPACE: KeyId = 0xEF08;
/// Tab.
pub const TAB: KeyId = 0xEF09;
/// Return.
pub const RETURN: KeyId = 0xEF0D;
/// Pause.
pub const PAUSE: KeyId = 0xEF13;
/// Scroll Lock.
pub const SCROLL_LOCK: KeyId = 0xEF14;
/// Escape.
pub const ESCAPE: KeyId = 0xEF1B;
/// Home.
pub const HOME: KeyId = 0xEF50;
/// Left.
pub const LEFT: KeyId = 0xEF51;
/// Up.
pub const UP: KeyId = 0xEF52;
/// Right.
pub const RIGHT: KeyId = 0xEF53;
/// Down.
pub const DOWN: KeyId = 0xEF54;
/// Page Up.
pub const PAGE_UP: KeyId = 0xEF55;
/// Page Down.
pub const PAGE_DOWN: KeyId = 0xEF56;
/// End.
pub const END: KeyId = 0xEF57;
/// Print Screen.
pub const PRINT: KeyId = 0xEF61;
/// Insert.
pub const INSERT: KeyId = 0xEF63;
/// The menu key.
pub const MENU: KeyId = 0xEF67;
/// AltGr (Mode_switch).
pub const ALT_GR: KeyId = 0xEF7E;
/// Num Lock.
pub const NUM_LOCK: KeyId = 0xEF7F;
/// Keypad Enter.
pub const KP_ENTER: KeyId = 0xEF8D;
/// Keypad `*`.
pub const KP_MULTIPLY: KeyId = 0xEFAA;
/// Keypad `+`.
pub const KP_ADD: KeyId = 0xEFAB;
/// Keypad `-`.
pub const KP_SUBTRACT: KeyId = 0xEFAD;
/// Keypad `.`.
pub const KP_DECIMAL: KeyId = 0xEFAE;
/// Keypad `/`.
pub const KP_DIVIDE: KeyId = 0xEFAF;
/// Keypad 0; the digits follow in order to Keypad 9 (0xEFB9).
pub const KP_0: KeyId = 0xEFB0;
/// Keypad `=`.
pub const KP_EQUAL: KeyId = 0xEFBD;
/// F1; F2 to F24 follow in order (F24 is 0xEFD5).
pub const F1: KeyId = 0xEFBE;
/// Left Shift.
pub const SHIFT_L: KeyId = 0xEFE1;
/// Right Shift.
pub const SHIFT_R: KeyId = 0xEFE2;
/// Left Control.
pub const CONTROL_L: KeyId = 0xEFE3;
/// Right Control.
pub const CONTROL_R: KeyId = 0xEFE4;
/// Caps Lock.
pub const CAPS_LOCK: KeyId = 0xEFE5;
/// Left Meta.
pub const META_L: KeyId = 0xEFE7;
/// Right Meta.
pub const META_R: KeyId = 0xEFE8;
/// Left Alt (a Mac's left Option).
pub const ALT_L: KeyId = 0xEFE9;
/// Right Alt.
pub const ALT_R: KeyId = 0xEFEA;
/// Left Super (a Mac's left Command).
pub const SUPER_L: KeyId = 0xEFEB;
/// Right Super.
pub const SUPER_R: KeyId = 0xEFEC;
/// Delete (forward).
pub const DELETE: KeyId = 0xEFFF;

/// The X11 keysym `ISO_Level3_Shift`, which Synergy reads as AltGr.
const KEYSYM_ISO_LEVEL3_SHIFT: u32 = 0xFE03;

/// Whether an ID is a named key rather than a character.
#[must_use]
pub const fn is_named(id: KeyId) -> bool {
    id >= 0xE000 && id <= 0xEFFF
}

/// The character an ID stands for, if it is a printable character.
#[must_use]
pub fn character(id: KeyId) -> Option<char> {
    if is_named(id) {
        return None;
    }
    char::from_u32(id).filter(|c| !c.is_control())
}

/// Whether an ID is a modifier key.
#[must_use]
pub const fn is_modifier(id: KeyId) -> bool {
    matches!(
        id,
        SHIFT_L
            | SHIFT_R
            | CONTROL_L
            | CONTROL_R
            | META_L
            | META_R
            | ALT_L
            | ALT_R
            | SUPER_L
            | SUPER_R
            | ALT_GR
    )
}

/// Whether an ID is a lock that toggles: Caps Lock, Num Lock, Scroll Lock.
#[must_use]
pub const fn is_lock(id: KeyId) -> bool {
    matches!(id, CAPS_LOCK | NUM_LOCK | SCROLL_LOCK)
}

/// The named ID of an X11 keysym, for the keys that are not characters:
/// the keysym less 0x1000 for the range 0xFF00 to 0xFFFF, and AltGr for
/// `ISO_Level3_Shift`.
#[must_use]
pub const fn named_from_keysym(keysym: u32) -> Option<KeyId> {
    match keysym {
        KEYSYM_ISO_LEVEL3_SHIFT => Some(ALT_GR),
        0xFF00..=0xFFFF => Some(keysym - 0x1000),
        _ => None,
    }
}

/// The X11 keysym of a named ID: the ID plus 0x1000, and `ISO_Level3_Shift`
/// for AltGr, the keysym layouts give the AltGr key.
#[must_use]
pub const fn keysym_of_named(id: KeyId) -> Option<u32> {
    match id {
        ALT_GR => Some(KEYSYM_ISO_LEVEL3_SHIFT),
        0xEF00..=0xEFFF => Some(id + 0x1000),
        _ => None,
    }
}

/// Pairs of an evdev code and the named ID of the key at that position on
/// a PC keyboard, for the keys whose meaning no layout changes.
const NAMED: &[(u16, KeyId)] = &[
    (1, ESCAPE),
    (14, BACKSPACE),
    (15, TAB),
    (28, RETURN),
    (29, CONTROL_L),
    (42, SHIFT_L),
    (54, SHIFT_R),
    (55, KP_MULTIPLY),
    (56, ALT_L),
    (58, CAPS_LOCK),
    (69, NUM_LOCK),
    (70, SCROLL_LOCK),
    (71, KP_0 + 7),
    (72, KP_0 + 8),
    (73, KP_0 + 9),
    (74, KP_SUBTRACT),
    (75, KP_0 + 4),
    (76, KP_0 + 5),
    (77, KP_0 + 6),
    (78, KP_ADD),
    (79, KP_0 + 1),
    (80, KP_0 + 2),
    (81, KP_0 + 3),
    (82, KP_0),
    (83, KP_DECIMAL),
    (96, KP_ENTER),
    (97, CONTROL_R),
    (98, KP_DIVIDE),
    (99, PRINT),
    (100, ALT_R),
    (102, HOME),
    (103, UP),
    (104, PAGE_UP),
    (105, LEFT),
    (106, RIGHT),
    (107, END),
    (108, DOWN),
    (109, PAGE_DOWN),
    (110, INSERT),
    (111, DELETE),
    (117, KP_EQUAL),
    (119, PAUSE),
    (125, SUPER_L),
    (126, SUPER_R),
    (127, MENU),
];

/// The evdev codes of F1 to F24.
const FUNCTION_CODES: [u16; 24] = [
    59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88, 183, 184, 185, 186, 187, 188, 189, 190, 191,
    192, 193, 194,
];

/// The named ID of the key at an evdev position, `None` for a character key.
#[must_use]
pub fn named_for_evdev(code: u16) -> Option<KeyId> {
    if let Some(i) = FUNCTION_CODES.iter().position(|c| *c == code) {
        return u32::try_from(i).ok().map(|i| F1 + i);
    }
    NAMED.iter().find(|(c, _)| *c == code).map(|(_, id)| *id)
}

/// The evdev position of a named ID on a PC keyboard.
#[must_use]
pub fn evdev_for_named(id: KeyId) -> Option<u16> {
    if (F1..F1 + 24).contains(&id) {
        return FUNCTION_CODES.get((id - F1) as usize).copied();
    }
    NAMED.iter().find(|(_, n)| *n == id).map(|(c, _)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_ids_are_synergy_s_and_follow_the_keysyms() {
        // A few of Input Leap's key_types.h values.
        assert_eq!(
            (BACKSPACE, RETURN, ESCAPE, LEFT),
            (0xEF08, 0xEF0D, 0xEF1B, 0xEF51)
        );
        assert_eq!(
            (F1, SHIFT_L, CONTROL_L, CAPS_LOCK),
            (0xEFBE, 0xEFE1, 0xEFE3, 0xEFE5)
        );
        // X11: BackSpace is 0xFF08, Super_L 0xFFEB.
        assert_eq!(named_from_keysym(0xFF08), Some(BACKSPACE));
        assert_eq!(keysym_of_named(SUPER_L), Some(0xFFEB));
        assert_eq!(named_from_keysym(0xFE03), Some(ALT_GR));
        assert_eq!(keysym_of_named(ALT_GR), Some(0xFE03));
        assert_eq!(named_from_keysym(0x0061), None, "a is a character");
    }

    #[test]
    fn characters_and_named_keys_are_told_apart() {
        assert_eq!(character(u32::from('@')), Some('@'));
        assert_eq!(character(u32::from('é')), Some('é'));
        assert_eq!(character(RETURN), None);
        assert_eq!(character(0x0D), None, "a control character is not typed");
        assert!(is_named(F1) && !is_named(u32::from('a')));
        assert!(is_modifier(ALT_L) && !is_modifier(CAPS_LOCK) && is_lock(CAPS_LOCK));
    }

    #[test]
    fn named_keys_have_their_pc_positions_both_ways() {
        for code in 1..300u16 {
            if let Some(id) = named_for_evdev(code) {
                assert_eq!(evdev_for_named(id), Some(code), "{code}");
            }
        }
        assert_eq!(named_for_evdev(59), Some(F1));
        assert_eq!(named_for_evdev(88), Some(F1 + 11), "F12");
        assert_eq!(named_for_evdev(194), Some(F1 + 23), "F24");
        assert_eq!(named_for_evdev(30), None, "A is a character key");
        assert_eq!(evdev_for_named(SUPER_L), Some(125));
    }
}
