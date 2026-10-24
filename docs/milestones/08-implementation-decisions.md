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
is the bound: when a frame has been written and more have piled up, the backlog is brought up
to date rather than sent whole. From the newest keyframe in it on, the frames form a chain a
decoder can follow, so they all go and only the frames before that keyframe are dropped; a
backlog with no keyframe is dropped whole, a keyframe is asked for, and deltas are dropped
until it comes, since a delta whose predecessor was dropped would corrupt the picture. An
earlier rule that kept only the newest frame dropped a keyframe and the delta after it when
the two arrived together (found under LP-151). The two-daemon test cannot make a loopback
stream fall behind for long, so the rule is checked by a unit test of `select_backlog`; the
run with the real devices over wireless is where it shows.

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

## LP-149: Linux capture and encoding, the sharing notification and `lpview`

**Child processes, not linked libraries.** The plan proposed GStreamer for the pipeline.
Linking GStreamer or ffmpeg would make every Linux daemon need those libraries to start,
clipboard users included, and this machine's GStreamer has the PipeWire source but no H.264
encoder or decoder, while its ffmpeg has `libx264`, `h264_vaapi` and the decoder. So a share
is a pipeline of children whose bytes the daemon reads, and each tool is optional: a machine
without ffmpeg's H.264 encoder does not advertise `shares`, one without its decoder does not
advertise `views`, and the probe that decides runs once at start (`screen sharing tools
probed` in the log). On the portal path `gst-launch-1.0` runs `pipewiresrc` on the portal's
PipeWire remote, passed as an inherited descriptor because `pre_exec` would need unsafe
code, scaling and pacing to the agreed size and rate with `keepalive-time` so a still screen
keeps producing frames; its raw BGRx output feeds ffmpeg. VA-API is used when a two-frame
test encode on the render node succeeds, which it does not on this machine, so `libx264`
(`ultrafast`, `zerolatency`) is what was measured.

**Frames are split at access unit delimiters.** ffmpeg writes an Annex B byte stream with a
delimiter before every frame (`aud=1`, or `-aud 1` on VA-API) and the parameter sets before
every keyframe (`repeat-headers=1`, or the `dump_extra` filter), and the daemon cuts the
stream at the delimiters; a unit with an IDR slice is a keyframe. A keyframe comes every
second (`-g` at the frame rate) because a child process cannot be asked for one: the sync's
keyframe request is logged and answered by the next periodic one, so a dropped frame costs
at most a second of picture.

**`wlr-screencopy` is the fallback, and the way the share is tested.** Where the portal or
GStreamer's PipeWire element is missing, or `LIGATURE_SCREEN_CAPTURE=screencopy` is set,
`ligatured screen-helper capture OUTPUT RATE` copies the output into shared memory at the
agreed rate and writes packed frames after a `LPRAW <width> <height> <pix_fmt>` line; an
output that changes size mid-share is scaled back to the first size, so the encoder never
stops (a nested compositor's window being re-tiled did exactly that). `screen-helper list`
gives the outputs, their logical geometry from `xdg-output`, and whether screencopy exists;
the daemon lists every five seconds, and an output's name is its display id, the one at the
layout's origin primary. The ignored test `screen_nested` shares a nested Hyprland's output
to a second daemon and checks with ffprobe that every frame decodes at the agreed size.

**The portal decides the display, and its token is kept.** The portal's own picker chooses
what is shared, so on that path a viewer's display choice is a request the picker may
override; the stream's position names the output actually shared. The restore token comes
back with persistence until revoked and is kept in the store's `local_settings` (schema step
4), so the picker shows once. The portal session's `Closed` signal ends the share with
`consent`.

**A shared display has a region for input.** A viewer drives with positions as fractions of
the shared display, while the injector places the pointer in fractions of the whole layout.
The capture reports where the display lies (`Capture::region`), and the screen sync sets
that region on the input sync for the viewer's key while the share runs, so a click on a
second monitor's picture lands on that monitor.

**The sharing notification.** A share posts "Sharing <display> with <device>" through
`org.freedesktop.Notifications` with a Stop sharing action, resident and without a timeout,
carrying the daemon's own origin hint so the notifications monitor never sends it to the
person's other devices; the action ends the share with `sharer`, and the notification is
closed when the share ends any way.

**`lpview` decodes with ffmpeg and draws with softbuffer.** The viewer is a new binary crate
with a winit window drawn through softbuffer, no GPU or toolkit dependency; ffmpeg decodes
the frames the daemon sends as `screen.started` and `screen.frame` events into raw pictures
at the share's size, which are scaled to the window with letterboxing. A first click on the
picture asks the daemon to drive (`input.drive`, an absolute entry where the click landed,
not sent as a click); from then on positions over the picture, buttons, the wheel and keys
go as `input.send` batches every 8 ms. Keys are mapped from winit's layout-independent key
codes to evdev codes by a table, the same on Linux and macOS. Both Control keys, losing focus
or closing the window let go, and a drive started over a control connection ends when that
connection closes, so a viewer that is killed leaves nothing driven. `LPVIEW_DEBUG` prints
every window event. A Mac daemon with ffmpeg advertises `views`, so `lpview` there watches a
Linux desktop before the Mac's own capture (LP-150) exists.

**Checked with two nested compositors.** A sharing daemon on one nested Hyprland
(`lp-test`, screencopy, the real injector) and a viewing daemon with `lpview` on another
(`lp-view`), clicks and keys injected into the second through the input helper, avoid any
feedback loop: the picture showed, a click took control and placed the shared pointer at the
picture's centre, a move to a quarter of the picture put it at a quarter of the shared
screen, F9 fired a binding there, both Control keys released, and killing `lpview` ended both
the drive and the share. The loopback round trip in a debug build was 35 ms, most of it the
injector's Wayland round trip per event. Not exercised: the portal path, which needs the
owner at the picker, and VA-API, which this machine refuses.
