# 0045. A shared display reports its scale, so a viewer can show it at its actual size

**Status:** Accepted (by the owner's direction, 2026-10-25)
**Date:** 2026-10-25

## Context

The screen family (chapter 05 section 3h) lists a sharer's displays with their size in
pixels, and a viewer asks for the largest picture it wants. Every desktop and phone draws
in logical points that the platform maps to pixels at a scale it chooses: two pixels to the
point on a Retina Mac, whatever the user set per monitor on a Wayland compositor (one, one
and a half, two), and the display density over 160 on Android (three on a Pixel 8 Pro).

The owner asked for a viewer that shows a share at its actual size, so that nothing is
scaled to fit the window, and so that the size accounts for each end's scale: a Retina Mac
shared to a Linux monitor at scale one should appear at the size its windows have on the Mac,
not at twice that. The viewer knows its own scale. It cannot know the sharer's from a size
in pixels alone, since 2880 by 1800 is a 1440 by 900 Retina screen or a 2880 by 1800 screen
at scale one.

## Decision

1. **A display carries its scale.** Each display in the sharer's `displays` answer has an
   optional `scale`: the display's pixels per logical point, a number greater than zero, as
   the platform reports it (a backing scale factor, a compositor's output scale, a density).
   An absent `scale` means 1, so a sharer that does not know it, or one built before this
   field, is taken at one pixel to the point.
2. **The viewer sizes its request from it.** A viewer that wants the actual size asks in
   `start` for the display's size in points times its own scale, in its own pixels. The
   sharer settles as it always does (never larger than the display, the aspect kept), and
   the viewer draws each picture pixel at the ratio of its own scale to the sharer's: one
   to one where the two are the same or the sharer scaled down, and a whole multiple where
   the viewer's scale is higher and the sharer could not send more pixels than it has.
3. **Nothing else changes.** How a viewer fits, scrolls or scales the picture remains its
   own choice; the protocol carries only the fact it needs.

## Consequences

- A share between two devices of different scales can be shown at the size it has on the
  sharer, with no scaling on the viewer where the sharer can scale down, which keeps text
  sharp.
- The field is additive: a decoder that ignores it treats every display as scale one, which
  is what the family meant before.
- A display whose scale changes during a session reports the new one at the next `displays`
  request. A running share keeps the size it started with.
- The schema's `display` and chapter 05 section 3h change with this ADR, and stay marked
  draft while ADR 0008 is Proposed.
