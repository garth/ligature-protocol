# Milestone 8 implementation decisions

Choices that are not part of the protocol but that a screen watched on another device needs
settled. Each is recorded under the ticket that made it; the protocol-level choices are in
[08-remote-desktop.md](08-remote-desktop.md) section 3.

## LP-148: the daemon core, `lpctl screen` and the two-daemon test

**The viewer decodes; the daemon carries encoded frames.** The daemon has no decoder and no
window, and a shared buffer of decoded pictures would be a platform API of its own on each
desktop. So a viewer (`lpview` on the desktops, the app on the phone) asks the daemon to
watch a device and receives the encoded frames as they were sent: over the control socket as
`screen.started` and `screen.frame` events, each event one chunk within the control channel's
own 1 MiB frame, in order, the response coming when the share ends; on the phone through the
node. The viewer feeds the bytes to its platform's decoder and draws. `lpctl screen start`
writes the same bytes to a file, which `ffplay -f h264` plays, so a share is checked without
a viewer program.

**Two traits again, either side optional.** `ligatured::screen::Capture` is the sharer's
platform: it lists displays and starts a share, which hands back a channel of encoded frames
and a control channel for keyframe requests and the stop; it may block on the platform's
consent dialog, so the sync calls it off the runtime. The viewer side is not a trait: it is
the daemon's own `ScreenParams` (what the local decoder takes), set when the family is
attached, and a `Watching` handle per share. A daemon attaches a capture, viewer params, or
both.

**Drop rather than queue, then a keyframe.** The capture's channel is unbounded and the sync
is the bound: when a frame has been written and more than one is waiting, the sync sends only
the newest, counts the rest as dropped, asks the encoder for a keyframe, and drops every
delta frame until that keyframe arrives, since a delta frame whose predecessors were dropped
would corrupt the picture. The two-daemon test cannot make a loopback stream fall behind, so
this path is exercised only by the counters; the run with the real devices over wireless is
where it shows.

**One share per display, many per device.** A second `start` for a display already shared is
refused `busy`; another display of the same device shares at the same time on its own stream.
A `stop` from the sharer's side (`lpctl screen stop`, with a device or for everything) ends
the stream tasks through a channel each share holds.

**Capture times are the capture's.** A frame's `at` is what the platform stamped, in
microseconds since the share started; a capture that stamps nothing (`at` of zero) gets the
sync's clock. The frame rate is clamped to 60 and the picture fitted to the display's aspect
with even dimensions, as encoders want.

**Sixty seconds for `started`.** The viewer waits a minute for `started` or `refused`, since
the sharer may be showing a consent dialog across the room; `displays` waits ten seconds.

**Fakes behind `LIGATURE_FAKE_SCREEN=1`.** `FakeCapture::new` takes fixed displays and emits
only what a test gives it; `FakeCapture::generating` has one 1920 by 1080 display and paces
synthetic access units at the agreed rate, a 200 KB keyframe every two seconds and on
request, 8 KB deltas between. The daemon attaches the generating capture and viewer params
that take anything when the variable is set, so `lpctl screen` can be tried end to end on
one machine. Without it a daemon offers no screen until LP-149 and LP-150.
