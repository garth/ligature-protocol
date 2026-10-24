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

## LP-143: Linux capture and injection

**Capture goes through the InputCapture portal, not a layer-shell surface.** The plan's
first check was whether Hyprland's portal backend implements `InputCapture`; on this
machine (`xdg-desktop-portal-hyprland` 1.4.1, Hyprland 0.56) it does, so `LinuxCapture` in
the daemon creates a portal session, asks for the zones (the outputs' logical rectangles),
sets a pointer barrier on each edge the layout maps (`barriers_for`: on the outer boundary
of the union of zones, so a layout of two monitors gets one barrier per monitor that touches
the extreme), and enables the session. The compositor's `Activated` signal, with the barrier
and the cursor position, is the edge hit; the captured keyboard and mouse then arrive over
libei (`reis`, as a receiver), and `Release` with a position gives them back and places the
cursor just inside the edge. None of this needs the compositor's socket, so it runs in the
daemon; the libei event stream is not `Send`, so it runs on a thread of its own with a
single-threaded runtime. `Deactivated` and `Disabled` from the compositor end a drive as a
release would (`CaptureEvent::Lost`), and `ZonesChanged` re-arms. The portal wrapper is
`ashpd`, which still builds on zbus 5 beside the daemon's zbus 6; a later dependency update
can bring the two together.

**Injection is a helper, as the clipboard is.** `ligatured input-helper` binds a
`zwlr_virtual_pointer_v1` and, on the first key, a `zwp_virtual_keyboard_v1` with the seat's
own keymap (read from `wl_keyboard`, or the US layout by `include` when the seat has none),
so evdev codes land as the layout says. A drive begins with an absolute motion against a
fixed extent, so fractions place the pointer without knowing the layout; relative motion,
buttons, and discrete and smooth scroll map to the pointer's requests; the helper lifts
every key and button it holds when the drive ends and when the daemon goes away. The screen
it reports is the layout's logical bounding box from `xdg-output`.

**Text is a short-lived keyboard of its own.** Hyprland reads a virtual keyboard's keymap
when it is set and applies the configured layout at creation, so a keymap swapped on the
running keyboard is not what a `wtype`-style sequence needs; instead a `text` event creates
a new virtual keyboard whose keymap names each distinct character as a Unicode keysym
(`U00E9`), presses and releases each, and destroys the keyboard. `xkbcommon` reads that
keymap without any system data, and the compositor sends the keyboard's keymap to the
focused client, which is how the character arrives.

**Measured in a nested Hyprland.** The ignored test `input_nested` drives a daemon's
injector into a nested compositor on its own socket (launcher: a Python script that binds
`$XDG_RUNTIME_DIR/lp-test` and execs `Hyprland --socket lp-test --wayland-fd N -c conf`):
an edge entry lands at the left edge half way down, a motion of 100 controller pixels moves
the nested pointer by exactly the width-scaled 128, so Hyprland applies no acceleration of
its own to a virtual pointer and the sync's tracked position does not drift; F9, a click,
the characters `q` and `é` and then F10 reach the compositor, and the return by the edge
releases the controller's capture at the right height. Two facts for the run: Hyprland
resolves binds by keycode unless `input:resolve_binds_by_sym` is set, so a typed character
is seen by a bind only with that option (an application sees it either way); and the
compositor makes a new virtual keyboard the seat's active one, so the helper creates the
keyboard only when the first key is injected.

**Not yet exercised: a live capture.** The portal session, its zones and the libei
connection were checked against this machine's session with no edge mapped (no barrier,
nothing captured); arming an edge on the owner's desktop and crossing it is the run with the
real devices (LP-146), since a captured pointer cannot be tested unattended.
