# 0043. Keys carry what they mean as well as where they are, after Synergy

**Status:** Accepted (by the owner's direction, 2026-10-25)
**Date:** 2026-10-25

## Context

The input family (chapter 05 section 3g, milestone 7) sends a key as its Linux evdev code,
which names a physical key as USB HID does, and the driven device applies its own layout.
That is what a keyboard plugged into the driven device would do, and it fails where the two
devices' keyboards or layouts differ, which between a Mac and a PC is the usual case:

- What is typed is not what the controller's keycap says. A Mac with a British layout
  driving a Linux machine set to US types `"` for the keycap's `@`, and the key under
  Escape is `§` on one and `` ` `` on the other.
- Modifiers travel by position too, so they mean whatever the driven layout has at that
  position. On the owner's Linux machine, whose options swap Alt and Super and make the
  Caps Lock position Compose, the Mac's Option arrived as Super and its Caps Lock as Compose.
- A shortcut is a position, so on a driven device with another layout Control and the key
  where C is on the controller is not Copy.

Keyboards cannot do better: USB HID has a Unicode usage page that no major operating system
implements, and keyboards that seem to need no layout setting either remap positions on the
assumption that the host is set to US, or type each character through the host's Unicode
entry method (QMK, ZMK), which differs per system and breaks shortcuts. Both ends of a drive
are software that knows its own layout, so the protocol can carry the meaning.

The model for that has been in use for more than twenty years. Synergy, a reimplementation
of Chris Schoeneman's CosmoSynergy, sends each key as a key ID (what the key means on the
sending machine), the modifier mask, and the physical key; Symless's Synergy 1.9 was forked
as Barrier, and Barrier as Input Leap, which keeps the same key message (`DKDN`, `DKUP`) and
key IDs. The owner asked for that model to be copied, since it is tried and tested. Input
Leap's source is the reference for the details here: `src/lib/inputleap/protocol_types.h`
and `key_types.h` for the message and the IDs, `KeyMap.cpp` for how a receiver finds a key,
`platform/OSXKeyState.cpp` for how a Mac computes an ID.

## Decision

1. **A `key` event carries `id` and `mods`.** Beside `code` and `pressed`:
   - `id` is Synergy's key ID: for a character, its Unicode scalar value; for a named key,
     the value in the private-use block U+E000 to U+EFFF that Synergy gives it, which for
     the keys of the X11 keysym range 0xFF00 to 0xFFFF is the keysym less 0x1000
     (BackSpace 0xEF08, Return 0xEF0D, Escape 0xEF1B, Left 0xEF51, F1 0xEFBE, Shift_L
     0xEFE1, Control_L 0xEFE3, Caps_Lock 0xEFE5).
   - `mods` is Synergy's modifier mask of the controller at the event: Shift 0x1, Control
     0x2, Alt 0x4, Meta 0x8, Super 0x10, AltGr 0x20, CapsLock 0x1000, NumLock 0x2000,
     ScrollLock 0x4000.
   - `code` stays the physical key. A release is matched to its press by `code`, so a
     release needs no `id`, as in Synergy, where the key ID of a release may differ from its
     press's.
2. **The controller says what the key means on its own layout.** A named key, modifiers
   included, sends its named ID. A character key sends the character its layout produces
   with Shift and Caps Lock applied and with Control, Alt (Option on a Mac) and Super
   (Command on a Mac) ignored, as Synergy's Mac primary does, so Option is Alt and a
   shortcut's ID is its letter. A dead key is composed on the controller: its press sends
   nothing, and the key that completes it sends the composed character. A Mac's Command is
   Super and its Option is Alt, as in Synergy.
3. **The driven device finds its own key for the meaning.** For a press with an `id`:
   - A modifier's ID presses the driven device's own key for that modifier, so Alt is Alt
     whatever position it has there; Caps Lock and Num Lock toggle the driven device's
     lock rather than press a key.
   - With a command modifier in `mods` (Control, Alt, Meta or Super), it presses the key
     that produces the ID with the same Shift, leaving the held modifiers as they are:
     "we're after the right button not the right character" (Synergy's `mapCommandKey`).
     Synergy counts AltGr among them; here it is not, since it chooses a character's level
     as Shift does, so a character typed with AltGr is typed exactly.
   - Otherwise it presses the key and level that produce the ID exactly, setting the level
     modifiers that level needs (Shift, AltGr) for the press and restoring them after
     (Synergy's `mapCharacterKey`).
   - If no key produces the ID, a character is typed as with `text` (an addition to
     Synergy, which drops it), and anything else falls back to `code`.
   A press without an `id` is placed by `code` as before.
4. **Old and new interoperate.** Decoders ignore unknown fields, so a device of an earlier
   build places every key by `code`, and a controller of an earlier build sends no `id`, so
   its keys are placed by `code`. `text` stays for a controller whose keyboard produces
   characters rather than keys, the phone's.

## Consequences

- What is typed is what the controller's keycap and layout say, on any driven layout; a
  shortcut is the shortcut of the same letter; modifiers keep their meaning across layouts
  that move them.
- Each platform needs its layout as data: the Mac reads it with `UCKeyTranslate`; Linux
  needs xkbcommon on both sides (the keymap libei hands the capture, the seat keymap the
  injector types through), which the daemon takes as the `xkbcommon` crate on Linux and the
  CI and release jobs install as `libxkbcommon-dev`; `lpview` takes the meaning from its
  window system's logical key.
- As in Synergy, characters that need Option on a Mac are not sent as characters; Option
  is Alt. A character the driven layout lacks is typed through the `text` mechanism, which
  on Linux swaps in a keymap for it.
- The schema's `key` event and chapter 05 section 3g change; milestone 7's decision 4,
  that keys travel as positions only, is amended by this record.
