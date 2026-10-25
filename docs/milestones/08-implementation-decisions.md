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
a viewer program. A viewer that closes its connection ends the watch, and through it the
share, at once, frames or not: a closed connection's writer is given half a second to flush
and then stopped, and the share's carriage watches for that. It used to notice only when the
next frame failed to reach the viewer, so a share whose capture had stalled outlived its
viewer and refused the next as `busy`.

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
at the share's size, which are scaled to the window with letterboxing. The decoder passes
every picture through as it comes (`-fps_mode passthrough`) and does not use `-fflags
nobuffer`: a raw stream has no timestamps and VideoToolbox's carries no frame timing, so
ffmpeg fitted the Mac's pictures to a rate it guessed and kept 2 of 240, and `nobuffer`
threw away the packets read while probing, so nothing showed before the second keyframe.
Fed frame by frame at 30 a second the decoder lags two frames. Closing the window kills
the decoder before the process ends, which otherwise wrote on into a closed pipe and reported
each failure. Keys: macOS reports Caps Lock only as its state changes, pressed when it turns
on and released when it turns off, which left the Linux machine's Caps Lock on; on a Mac each
change is sent as a press and a release. And every modifier sent as pressed is checked
against the window's modifier state, so one whose release the window never reported is
released on the driven device; the Linux machine sometimes got capitals from `lpview` on the
Mac with Caps Lock off. A first click on the
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

## LP-151: the phone shares its screen and watches a desktop's

**The capture is the app's; the node waits on it.** Only an app may capture an Android screen,
after the user consents in the media projection dialog, and only the app runs `MediaCodec`.
So the node's capture (`HostScreen`) asks the host when a viewer starts a share
(`screen_share_requested`), waits up to a minute for `screen_share_started`, then takes the
encoded access units the app hands it (`screen_share_frame`) and the end
(`screen_share_ended`); the viewer's keyframe request and the stop go back to the host. The
phone advertises `shares` once the app has said it has an H.264 encoder and the screen's
size, and `views` always, since every Android device decodes H.264.

**Consent every share, from where Android allows.** Android keeps an app in the background
from opening activities, so a request becomes a high-priority notification, "deskbox wants to
see your screen", whose tap opens the dialog; when the app is in front the dialog opens at
once. The dialog defaults to sharing one app, and the user picks the entire screen. On
consent a foreground service of the media projection type mirrors the screen into a virtual
display on the encoder's input surface; Android shows its cast indicator and the app's own
notification carries Stop sharing, which ends the share with `sharer`, while stopping the
projection from the system ends it with `consent`. A request answers only within 55 seconds,
so a tap on a stale notification opens nothing.

**The encoder is configured for viewers that join late.** Dimensions are rounded down to
multiples of 16, which encoders want (486x1080 asked becomes 480x1072, which the viewer scales);
a keyframe comes every second and on request (`PARAMETER_KEY_REQUEST_SYNC_FRAME`); a still
screen repeats its last frame at the frame rate. Encoders that refuse to put the parameter
sets before every keyframe (the emulator's) are configured without it, and the app puts the
codec configuration it saved before each keyframe that lacks it.

**Watching decodes into a surface.** The Watch screen is a `SurfaceView` at the share's
aspect, and the node hands each whole access unit to `WatchDecoder`, which drops deltas until
its first keyframe and asks for one. Touch on the picture drives the desktop with absolute
positions: the first touch starts the drive there (`input_start_at`), a tap clicks where it
lands, a drag moves the pointer with it, a touch held still then dragged drags with the button
down, two fingers scroll, and the field types. Touch events go through one queue for the watch
and the touchpad alike, drained by one worker task that sums consecutive motion and keeps
only the newest of consecutive positions, so a drag goes where the finger is rather than
replaying where it was.

**Checked on the emulator against a desktop daemon.** A desktop daemon sharing a nested
Hyprland's headless output (the window output stops rendering when its window is not on the
visible workspace) was paired into the emulator's test identity. The Watch screen showed the
desktop's picture, a tap at a quarter of the picture put the desktop's pointer at exactly a
quarter of its screen, and a drag settled at its end within about a second and a half on the
debug build. The desktop then watched the emulator: the dialog came up at once, and 45 frames
arrived and decoded at 480x1072, showing the emulator's screen with the cast indicator and the
sharing notification; stopping from the desktop stopped the service, and Stop sharing on the
phone ended a second share with `sharer`. The Pixel runs the build and advertises `input` and
`screen` to the owner's daemon; a share to or from it waits for a desktop of the owner's on
this build (the run, LP-152). Two findings: `adb shell input text` truncates long strings,
so the pairing offer went in two pieces with the emulator's keyboard disabled; and a share's
input region is computed when the share starts, so a layout that changes during the share maps
positions by the old one until the next share.

## LP-150: the Mac shares its screen

**ScreenCaptureKit into ffmpeg's VideoToolbox encoder.** The plan's VideoToolbox is reached
through ffmpeg, as Linux reaches VA-API, so the encoder arguments, the probe and the split of
the stream into frames are one module for both desktops (`screen::encode`, moved out of the
Linux backend). `lp-macos::screen` holds the ScreenCaptureKit code: the displays, the Screen
Recording check, and a stream of one display at the share's size and rate, the cursor
drawn in, delivered as BGRA rows packed tightly by an output object of the crate's own class
on a dispatch queue of its own. ffmpeg's `h264_videotoolbox` writes no access unit
delimiters and keeps the parameter sets in the stream's header, so two bitstream filters
put the parameter sets before every keyframe and then a delimiter before every frame
(`dump_extra=freq=keyframe,h264_metadata=aud=insert`). The order was checked on the Linux
machine with `libx264` made to keep its parameter sets in the header as VideoToolbox does:
every access unit started with a delimiter and every keyframe carried both sets. Where ffmpeg
has no working VideoToolbox encoder, `libx264` is used, and without ffmpeg the Mac does not
share.

**A pacer repeats the newest frame.** ScreenCaptureKit delivers a frame only when the
display changed. A thread writes the newest frame into ffmpeg at the agreed rate, the same
one again while the screen is still, so the encoder's keyframe every second reaches a viewer
who joins late, as `pipewiresrc`'s keepalive does on Linux.

**Consent is Screen Recording, applied on the next start.** A share asked for without the
permission asks macOS for it and is refused for `consent`; macOS applies the permission to
the next start of the process, so the daemon is restarted once after the user allows it, and
the install guide says how. The menu bar's sharing indicator is the platform's, so no
notification is posted, unlike Linux; stopping the share from the indicator ends it with
`consent`, any other end of the stream with `error`.

**Displays by their display id, the region by points.** A display's id on the wire is its
`CGDirectDisplayID`, its size the pixel size, its name "Built-in display", "Main display"
or "Display <id>", and the main display is the primary. With more than one display the
share's input region is the display's rectangle in points as a fraction of the displays'
bounding box (`input::geometry::region_of`, shared with the input backend), which is what the
Mac's injector places the pointer by.

**The agent is scheduled as an application.** The first run on the Mac shared about 12
frames a second instead of 30 and measured a 272 ms input round trip, while the network's
was 4.5 ms. The launchd agent had `ProcessType` `Background`, set when the daemon only
synchronised the clipboard, which launchd throttles so that it does not disturb the user.
It is now `Interactive`, the scheduling an application gets.

**`lpview` is unchanged.** It was written on winit, softbuffer and an ffmpeg decoder, which
all run on macOS, so the plan's AppKit viewer is not needed; it already type-checks for the
Mac.

**Built here, run on the Mac.** Type-checked and linted for `aarch64-apple-darwin` on the
Linux machine, as LP-144 was; built, linked and run on the Mac. To be recorded from that
run: the permission prompt and restart, the encoder the probe picks, the latency and CPU at
the default size and rate, and whether macOS's monthly reminder for screen recording
applications appears.
