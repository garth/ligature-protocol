# 0046. A Mac's right Option types characters, and its left Option stays Alt

**Status:** Accepted (by the owner's direction, 2026-11-08)
**Date:** 2026-11-08
**Supersedes:** the part of [ADR 0043](0043-keys-carry-their-meaning.md) decision 2 that makes
every Option key Alt and ignores it when a Mac chooses a key's character

## Context

ADR 0043 has the controller say what each key means on its own layout. On a Mac it ignores
Option when it chooses a character and sends Option as Alt, as Synergy does, so that
Option-letter shortcuts reach the driven device as Alt-letter. The consequence it named is
that characters that need Option on a Mac (é and ü through Option's dead keys, ©, ™, €
and the rest of the Option level) are never sent as characters.

In the run of milestone 7 with the real devices (LP-146) the owner, driving Linux from the
Mac, typed Option+E then E for é and got Alt+E and then e, and asked for those characters
to reach Linux. Option is both a Mac's third-level key and the key a PC's Alt sits on; one
key cannot be both. A PC keyboard settles it by side: the left Alt is Alt and the right one
is AltGr, which chooses the third level and is never a shortcut modifier, and ADR 0043
already treats AltGr that way on the driven side.

## Decision

1. **Left Option is Alt**, as ADR 0043 has it: a key typed with it is a shortcut, its ID the
   key's character without Option, and `mods` carries Alt.
2. **Right Option is the Mac's AltGr.** A key typed with right Option and no Control,
   Command or left Option is a character: the Mac chooses it with Option as well as Shift
   and Caps Lock, composes a dead key it starts as ADR 0043 decision 2 composes any, and
   sends the result as the key's ID with no Alt in `mods`. Right Option's own press and
   release are not sent, since the character carries all it meant.
3. With Control, Command or left Option held as well, right Option is ignored in choosing
   the character, as every Option is under ADR 0043, and the key is a shortcut.
4. The Mac tells the sides apart by the device-dependent bits of its event flags
   (`NX_DEVICELALTKEYMASK`, `NX_DEVICERALTKEYMASK`), which follow any swap of the modifier
   keys the user has set in System Settings.

Nothing changes on the wire: the `key` event, its `id` and `mods` are as ADR 0043 defines
them, and the driven device types an ID as it always has.

## Consequences

- Characters of the Option level and Option's dead keys reach Linux and other Macs from a
  Mac's right Option; Alt shortcuts from its left Option are unchanged.
- Right Option no longer gives Alt: a shortcut needing Alt, an Alt-arrow word move for one,
  is pressed with left Option.
- A Mac keyboard with one Option key (rare; most have two) has no AltGr; its Option stays Alt.
- The guide's keyboard page and chapter 05 section 3g say which Option does what.
