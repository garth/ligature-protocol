# Milestone 7 implementation decisions

Choices that are not part of the protocol but that one keyboard and mouse across a person's
devices needs settled. Each is recorded under the ticket that made it; the protocol-level
choices are in [07-shared-input.md](07-shared-input.md) section 3.

## LP-142: the daemon core, the layout, `lpctl input` and the two-daemon test

**Two traits, either side optional.** `ligatured::input::Capture` is the controller's own
keyboard and mouse: it reports when the pointer hits an armed edge, and while taken it
reports what the user does and keeps it from the local desktop. `Inject` is the driven
device's input system: it places the pointer, injects what it is sent and lifts everything
held when the drive ends. A daemon attaches whichever it has (`Daemon::attach_input`): the
desktops both, the phone neither, since its app drives through `InputSync::drive` and
`send` directly. The descriptor's `provides` comes from the capture and `accepts`, `absolute`
and `text` from the injector, re-read every second as the notifications are, so a platform
permission granted or revoked changes the manifest within a second.

**The controller scales motion.** Only the controller knows both widths (its own from the
capture, the driven screen's from `ready`), so it multiplies relative motion by the ratio of
the driven width to its own before sending; what travels is in the driven screen's pixels.
Section 3g and the schema say so; an earlier wording had the driven device scale, which it
cannot.

**The sync tracks the driven pointer.** The driven side keeps the pointer's position as
fractions, from the entry point and the motion it injects, and sends `leave` when that
position goes beyond the entry edge. The platform injector therefore never reports a
position, which the Wayland virtual pointer could not give anyway; if a compositor applies
acceleration to injected motion the tracked position drifts from the real cursor, which the
Linux task (LP-143) checks and, if it finds it, corrects in the helper. The motion that
crosses the edge is not injected.

**Batches per frame, `seq` every sixteenth.** Captured events are batched every 8 ms (or at
256 events, the schema's bound) into one `events` message. Every sixteenth batch, and the
first, carries `seq`; the driven device answers `injected` after the batch is in its input
system, and the controller keeps the last 256 round-trip samples, reported by
`lpctl input status` as the median and the worst.

**Everything held is lifted on every exit.** The driven side records the keys and buttons it
has pressed and calls the injector's `end` whenever the drive ends: on `leave`, on `release`,
when the stream ends and when the connection drops. The controller releases its capture on
the same four paths and puts the pointer back on its own edge at the height `leave` reported,
or where it was for a drive without an edge.

**The release chord is both Control keys.** The controller watches the keys it forwards; the
moment both Control keys are down it sends `release` and gives the input back. The chord
reaches the driven device as two presses, which its `end` lifts.

**The layout is three rows.** `input_layout(edge, device)` in the store (schema step 3,
LP-142), read at start and changed by `lpctl input layout set` and `unset`; the capture is
re-armed with the mapped edges on every change. A row names a device key, never a name, so
renaming a device changes nothing.

**Fakes behind `LIGATURE_FAKE_INPUT=1`.** `FakeCapture` and `FakeInject` record what the
sync asked of them and let a test hit an edge or feed events; the daemon attaches both when
the variable is set, so `lpctl input` can be tried without a platform backend. Without it a
daemon offers no input until LP-143 and LP-144 bring the platform sides.
