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

**Nothing holds the input without a drive.** On Linux the compositor captures the keyboard
and mouse the moment the pointer crosses a barrier, before the daemon decides anything, and
the pointer of a device driving this one crosses barriers too. So every edge hit that does
not become a drive gives the input back at once: a drive that does not start (the peer
gone, or refusing) and a hit while this device is driven across an edge by another desktop,
which decision 7 keeps from following its own layout, since that could loop. A device
driven by a touchpad or a viewer (an absolute entry) does follow its layout: their pointer
is like a mouse plugged into it, so the phone driving the Linux machine carries on across
its edge to the Mac. And a captured event that
arrives while nothing is driven gives it back at the first event, once per burst, whatever
let it happen. On 2026-10-24 the phone drove the Linux machine to its right edge, which led
to the Mac; the hit was ignored as the guard above says, the capture was never given back,
and the desktop had no keyboard or mouse until a forced restart.

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
can bring the two together. Its request and session proxies have no properties, and zbus
warns on each that it could not fill their cache, so the daemon's log filter sets
`zbus::proxy` to `error` whatever `RUST_LOG` says, unless `RUST_LOG` names zbus itself.

**The portal waits for the session.** The daemon starts with the user's systemd manager,
before the graphical session, and any call to the desktop portal starts
`xdg-desktop-portal`. Started then, the portal has no `XDG_CURRENT_DESKTOP` or
`WAYLAND_DISPLAY`, settles on fallbacks for the whole login, and offers no InputCapture or
ScreenCast to any application; after a restart of the owner's machine that is what happened
(LP-157). So the daemon reads the manager's environment from systemd, which starts nothing,
and touches no portal until it names a desktop and a Wayland display; input capture then
tries the portal again every half minute while it is missing, and the screen backend probes
it again every minute.

**Injection is a helper, as the clipboard is.** `ligatured input-helper` binds a
`zwlr_virtual_pointer_v1` and, on the first key, a `zwp_virtual_keyboard_v1` with the seat's
own keymap (read from `wl_keyboard`, or the US layout by `include` when the seat has none),
so evdev codes land as the layout says. A drive begins with an absolute motion against a
fixed extent, so fractions place the pointer without knowing the layout; relative motion,
buttons, and discrete and smooth scroll map to the pointer's requests; the helper lifts
every key and button it holds when the drive ends and when the daemon goes away. The screen
it reports is the layout's logical bounding box from `xdg-output`.

**The virtual keyboard reports its modifiers.** Hyprland takes a virtual keyboard's modifier
state from its `modifiers` messages alone, never from its key events, and merges it with the
other keyboards' for the applications and its own bindings. The helper sent keys only, so
Shift reached nothing as a modifier: the owner, driving the Linux machine from `lpview` on the
Mac, could not type shifted characters, and with the session's `shift:both_capslock_cancel`
a Shift press sometimes locked Caps. The helper now keeps the modifier state and reports it
after every modifier key. Each modifier key's role comes from the keysym on its first level
in the seat's keymap (Shift, Control, Alt or Meta as Mod1, Super as Mod4, ISO_Level3_Shift as
Mod5; Caps Lock and Num Lock lock Lock and Mod2), not from the keymap's `modifier_map`, which
as Hyprland serialises it puts Left Shift in Lock alone under that option. The nested test
binds Shift+F11 and F11: with the report the first fires, without it the second did.

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
real devices (LP-146), since a captured pointer cannot be tested unattended. The first
attempt in that run found the right and bottom barriers refused: Hyprland's portal takes a
barrier only on a zone's boundary, which for those edges is one past the last pixel
(`x + width`, `y + height`), and they had been on the last pixel.

## LP-145: the phone as a touchpad and keyboard

**The phone provides and never accepts.** The node attaches the input family with a
`Touchpad` capture, which advertises `provides`, has no edges and never reports an event,
and no injector, since Android lets an app inject nothing (chapter 08). The app drives
through four node calls: `inputTargets` (this person's connected devices whose descriptor
says `accepts`, with their names and whether they type text), `inputStart` (an absolute
entry at the middle of the chosen screen, answered with its size), `inputSend` (a batch of
`TouchEvent`s: motion in the driven screen's pixels, buttons, scroll in wheel steps, keys,
text) and `inputStop`. The node tells the host `InputEnded` when a drive ends for any
reason, so the touchpad screen closes when the desktop or the connection goes.

**Gestures on the screen, not in the node.** The touchpad screen's gesture loop turns one
finger into relative motion as the finger moved (the desktop's acceleration is not applied
again: the sync sends what the phone measured, in the driven screen's pixels), a short still
touch into a click, a touch held without moving into a press that lasts until the finger
lifts (a drag), and two fingers into wheel steps every forty finger pixels, with the sign of
a touchpad (fingers moving down scroll the page up). Buttons offer Backspace, Enter and a
right click, since an empty text field reports no backspace. The field sends each change as
`text` and empties itself, so the phone's keyboard, with its own layout and input method,
types characters rather than keys.

**One drive at a time, ended by the screen.** `inputStart` while a drive runs is refused;
leaving the screen (Done or Back) calls `inputStop`, which sends `release` and lifts
everything on the desktop. The node test `the_phone_is_a_touchpad_for_a_desktop` runs the
calls end to end against a daemon with the in-memory injector, built as the daemon tests
build their identities, and ends one drive by the screen and one by the daemon going away.

## LP-144: macOS capture and injection

**Both sides run in the daemon, inside `Ligature.app`.** macOS grants Accessibility and Input
Monitoring to a bundle by its identifier, and LP-94 put the daemon in one, so no helper is
needed. The Quartz calls live in `lp-macos::input`, the crate allowed `unsafe`; the daemon's
`input_macos` holds only safe code, and the arithmetic it needs (the key table, modifiers,
click counting, scroll units) is in `input::mac`, built and tested on every platform.

**Capture is a listening tap that watches the cursor, and a filtering tap per drive.** macOS
has no pointer barriers, so a listen-only session tap sees every mouse move and the daemon
reports an edge hit when the cursor arrives on an armed edge of the displays' bounding box,
once per arrival (`input::geometry::EdgeWatch`), so a pointer resting there while no drive
can start does not repeat. The zone arithmetic the Linux portal used moved to
`input::geometry` and is shared. A listening tap can hold nothing back. A filtering tap
can: every event of the session waits for it, and when its process lost Accessibility on
the owner's Mac, every click, gesture and key was held back until a forced restart. So the
filtering tap, at the head of the queue, exists only for the length of a drive: `take`
installs it and `release` removes it. While driving it swallows every event, the cursor is
frozen with
`CGAssociateMouseAndMouseCursorPosition` (the mouse still reports its motion, in points)
and hidden. Hiding it from a background process needs the window server's
`SetsCursorInBackground` connection property, which is not public: it is looked up by name
at run time, and without it the cursor stays visible but still. Key repeats are dropped,
since the driven device repeats for itself.

**Modifiers are flag changes.** macOS reports a modifier as `FlagsChanged` with the flags
after it, not as a key press. The side is read from the device-dependent bits (left and
right Shift, Control, Option and Command each have one); a source that sets none toggles.
Caps Lock reports only its new state, so it travels as a press and a release each time.

**Keys map by position through one table.** Virtual key codes name ANSI positions, as evdev
codes do, so the table pairs them one to one; Print Screen, Scroll Lock and Pause, which a
Mac lacks, are typed as F13 to F15, as macOS does with a PC keyboard; anything else
unmapped is dropped and logged at debug. Apple's ISO keyboards report the key left of 1 and
the key beside left Shift swapped relative to a PC's; the Mac reads each key event's keyboard
type (`KBGetLayoutType`) and swaps the two on an ISO keyboard both ways, as Linux's
`hid-apple` does (LP-160).

**Injection posts tagged events.** Events are posted at the HID level from an event source
whose user data is a fixed tag, and the tap passes tagged events without looking, so the
pointer a Mac is driven by never looks like its user's. The daemon core now also ignores
an edge hit while the device is driven (LP-142's decision 7, tested), since any capture
that watches the cursor would otherwise start a drive back. The pointer position is
tracked and kept on a display (`clamp_to_zones`, for layouts that are not rectangles); a
move while a button is down is posted as a drag; a button carries the click count, counted
against the user's double-click interval and a 4-point slop, which is what makes a double
click on the controller a double click on the Mac; every event carries the held modifiers'
flags.

**Scroll changes sign.** The wire's scroll is positive down and right, Quartz's up and
left. A wheel is posted in lines, smooth scrolling in pixels, and the fraction of a unit
waits for the next event. Captured scroll goes the other way, points from a trackpad and
lines from a wheel, with the user's natural-scrolling setting already applied.

**Text is a string on a key event.** `text` is posted as key down and up events carrying
the string, at most 20 UTF-16 units each, so any character types whatever the layout.

**The permissions gate the descriptor, read live.** At start the daemon asks for whichever of
the two permissions is missing, which macOS prompts for once each. `CGPreflightPostEventAccess`
and `CGPreflightListenEventAccess` answer from what the process held when it started: on the
owner's Mac they went on answering yes after the switch was turned off, which is how the
filtering tap outlived its permission. So a supervisor thread also reads both live every
half second, `AXIsProcessTrusted` for Accessibility and `IOHIDCheckAccess` for Input
Monitoring. A permission it has seen held and then sees gone is a withdrawal: the
listening tap stops, a drive's filtering tap goes with it, and the drive ends as a lost
capture. The supervisor starts the listening tap once both are granted and re-reads the
displays every two seconds. The family `provides` while the listening tap runs and
`accepts` while Accessibility is held, read live in the same way, since posted events are
dropped without it; the manifest re-reads both every second. On the
owner's Mac the switch sat in Privacy & Security under "Device Control and Data Access",
not under Accessibility.

**Built here, run on the Mac.** The code is type-checked and linted for
`aarch64-apple-darwin` on the Linux machine, with a stand-in C compiler since there is no
macOS SDK there; it is built, linked and run on the Mac. On the Mac, on 2026-10-24, the
Linux machine and the phone drove the Mac, and the Mac drove the Linux machine across its
edge with its cursor hidden, so the `SetsCursorInBackground` lookup works. The input round
trip from Linux was 7.8 ms median once the agent ran as `Interactive` (LP-150); as
`Background` it was 272 ms. Scrolling went the expected way in both directions, so the sign
change holds and posted scroll is not inverted a second time. The permissions do not
survive a rebuild: macOS ties them to the ad hoc signature, so a switch left on for the
previous build shows as on and does not apply, and the entries are reset with `tccutil` and
granted again, as the upgrade guide says. With Terminal's Secure Keyboard Entry on and its
window in front, the Mac's keys still reached the Linux machine it drove; a password field in
another application is not yet tried.
