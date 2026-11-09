# Milestone 10 implementation decisions

Choices that are not part of the protocol but that a camera used from another device needs
settled. Each is recorded under the ticket that made it; the protocol-level choices are in
[10-camera.md](10-camera.md) section 3.

## LP-206: the daemon core, one sync for both families

**The screen sync runs the camera family too.** The camera family is the screen family
under its own type names, so the daemon does not have a second machine for it: `ScreenSync`
takes a `Family`, which decides the stream's purpose, the descriptor, the names on the wire
and the listing, and a daemon attaches one sync for each family it offers
(`Daemon::attach_screen`, `Daemon::attach_camera`). Inside, a share is in the screen
family's shapes whichever family it belongs to: `lp_core::camera::CameraMessage` converts to
and from `ScreenMessage` at the stream (`start` and `started` name a `camera` where the
screen's name a display, an `offer`'s source has the kind `camera`), and everything else,
the negotiation, the chunking, the pacing and dropping, sound between the frames, casts,
offers and their decisions, is the same code. A camera stands as a display to that code, the
first listed being the primary, and it is never given an input region.

**The listing is the family's own.** `cameras` is the one message whose content differs, so
it does not go through the conversion: a capture of the camera family implements
`Capture::cameras`, the sharer answers the listing from it, and the asking side has
`ScreenSync::cameras`. `ScreenParams` gained `presents`, which only the camera's descriptor
means anything by.

**Presenting is a sink beside the viewer.** A `Presenter` makes the platform's camera device
and takes whole frames and packets; `ScreenSync::present` watches a camera as any viewer
does and hands what arrives to the presenter instead of to a client, so presenting needs no
window and no `lpview`. A cast that asks to be presented (`present` in its `offer`) is taken
by the presenter at once, with nobody asked, as any push between one person's devices is;
it needs a receiver whose descriptor says `presents`, while a cast to be shown needs one that
says `receives`. `presents` is advertised while the presenter says it is available.

**A camera's encoder is told what it is coding.** `Settings.kind` says camera, and the
encoder arguments follow: x264 codes to a constant rate factor of 23 under the rate cap
where a screen holds a quantiser of 18 with no cap, and the hardware encoders aim at their
bit rate as they do for a screen.

**The control socket and the tools.** `camera.list`, `camera.start`, `camera.present`,
`camera.cast`, `camera.accept`, `camera.decline`, `camera.stop` and `camera.status` are
handled by the screen's handler with the camera's sync, and their events are the screen's
shapes under `camera.started`, `camera.frame` and `camera.audio`. `lpctl camera` has `list`
(this device's own with no device), `present`, `cast` with `--present`, `start` into a file,
`stop` and `status`; `lpview <device> --camera [ID]` watches a camera with its microphone
and never drives, and the daemon starts `lpview --receive --camera` for a camera cast to be
shown.

**Fakes.** `FakeCapture::of_cameras` and `generating_camera`, and a `FakePresenter` that
counts what it is given; `LIGATURE_FAKE_CAMERA=1` attaches them, so `lpctl camera` runs end
to end on one machine. The two-daemon test `camera.rs` covers the listing, a watch by default
and by id with the microphone, `busy` and a camera the sharer lacks, a camera presented and
its end, a cast presented at once and one shown after an accept, a device that no longer
presents, and all of it over the control socket.

## LP-207: Linux, PipeWire's cameras and a camera node of its own

**Cameras are PipeWire's, listed by `pw-dump`.** `camera_linux` links no media library, as
the screen's capture links none. The daemon reads `pw-dump`'s JSON every ten seconds and
lists every node of the class `Video/Source` that offers raw pictures, the session's
preferred one first: its `node.name` is the camera's id, its description the name,
`api.libcamera.location` the facing, and its size the largest it offers within 1920 by
1080, at 30 a second where it says no rate. A laptop camera behind libcamera, such as this
machine's, appears nowhere else. The daemon reads PipeWire directly and not through the
Camera portal: the portal exists to let a sandboxed program reach what an unsandboxed one
already reaches, so its remote would only narrow what the daemon has, and there is no
picker in it to stand for consent. When PipeWire lists no camera, the first node of each
plain V4L2 device is asked for its formats through ffmpeg's `v4l2` input, once for each set
of devices, and read by ffmpeg.

**A share reads the camera it names and no other.** PipeWire's session manager links a
stream whose target is missing to the default camera, which was found the hard way: a
reader started for a test node that had failed to start opened this machine's own camera
instead. So `pipewiresrc` is given `node.dont-fallback` and `node.dont-reconnect`, with
which a missing camera gives "defined target not found" and no picture, at the start and
when the camera goes away during a share. The cameras are listed again when a share starts,
and a share whose camera has sent nothing for six seconds ends with `error`, since a reader
waiting on a camera that is not there waits for ever. The microphone is read the same way,
by `pw-record` with the same two properties into the encoder that takes a platform's
samples (`Sound::start_microphone`), not through PulseAudio's interface, which moves a
recorder to another source when its own goes: a microphone that is unplugged leaves the
share silent rather than listening through a device nobody chose.

**The pipeline.** `gst-launch-1.0 pipewiresrc ! videorate ! videoconvert ! videoscale` gives
raw BGRx pictures of the agreed size at the agreed rate, the camera's shape kept with black
borders, copied into the `LiveEncoder` by the screen's own relay. The camera advertises
`h264` alone: its picture is 4:2:0 at the source. The sharing notification is the screen's
with a camera's words and icon, and a share that cannot show it does not start.

**Presented as a PipeWire camera node, and on a loopback device where there is one.** The
plan left the choice between the kernel's `v4l2loopback` and a PipeWire node open. The node
is the default sink, since it needs nothing installed and no privilege: ffmpeg decodes the
share into raw 4:2:0 pictures and `gst-launch-1.0 fdsrc ! rawvideoparse ! queue ! pipewiresink
mode=provide` offers them as a node of the class `Video/Source` with the role `Camera`,
named `ligature_camera.<device>` and described "Ligature camera (*device*)". That is what
the Camera portal counts as a camera and what browsers that read cameras through PipeWire
list. The queue is leaky and the sink unsynchronised, so a slow reader costs pictures and
never delays the decoder, and a node nobody reads takes its pictures without blocking.
Where a `v4l2loopback` device exists, a second ffmpeg writes the same share to the first
one, for the programs that read V4L2 alone; the daemon cannot name that device, which is
named when the module is loaded. `presents` is advertised when either can be made. A node
whose name starts `ligature_camera.` is never listed as a camera to share.

**The microphone is a loopback pair.** A source that GStreamer offers is read cleanly by a
PipeWire client and badly through PulseAudio's interface, which is what browsers record
through: a fifth of the samples arrived. `pw-cat` as a source reads cleanly and stops taking
samples while nobody listens, so a program that starts listening would hear what queued.
`pw-loopback` makes an output, "Ligature microphone feed (*device*)", and a source,
"Ligature microphone (*device*)" (`ligature_microphone.<device>`), passes one to the other
in real time whether or not anything listens, and reads cleanly both ways. ffmpeg decodes
the share's Opus and plays it into the feed through its `pulse` output with a 40 ms buffer.
The player starts only once the feed exists and ends when the loopback does, since a player
whose output is missing is put on the default output, where the other device's microphone
would play on this machine's speakers. What arrived while the feed was being made is passed
over, so the microphone starts at the present. A source whose name starts
`ligature_microphone.` is never captured as this machine's own microphone. Neither the
camera node nor the pair is given a priority, so neither becomes the session's default over
the machine's own devices; this was checked before and after each run.

**A sink that is stuck is dropped.** Each chain of processes is fed from a thread of its own
through a queue of 120 frames or packets, so the share's task never waits on a pipe. A chain
that ends or whose queue fills is dropped, and the presenting ends when the last picture
sink does.

**What was run, and what was not.** `camera_live.rs`, ignored by default, makes a test
camera (a white ball on black) and a test microphone (a 440 Hz tone) in the session's
PipeWire, shares them between two daemons and presents them: 45 pictures of the ball read
back from the presented camera, four seconds of a 439 Hz tone from the presented microphone,
both nodes gone when the viewer stops, and the share ended with `error` when the test camera
was killed. A daemon run as a process lists this machine's built-in camera and says it
shares, views, receives and presents; the built-in camera was never started by the daemon.
Not run: this machine's own camera and microphone in a share, a browser or a call picking
the presented camera and microphone, the V4L2 fallback and the loopback device (this machine
has neither a plain V4L2 camera nor the module), whose commands are tested as text alone.

## LP-208: macOS, AVFoundation's cameras and no presenter

**A capture session with two delegates.** `lp_macos::camera` lists the cameras
(`AVCaptureDevice`, by unique id, with the name macOS shows, the position as the facing, and
the active format's size and rates), the system's default first, and captures one through an
`AVCaptureSession`: the camera's input and a video data output asked for BGRA, and the
default microphone's input and an audio data output when sound is wanted. The two outputs'
delegate protocols name the same method, so each has a small class of its own, both on one
serial queue. The cameras are listed with the older `devicesWithMediaType:`, deprecated for
the discovery session: that session takes device types, and the type for external cameras
is a constant macOS has only from 14, which a program that runs on 11 cannot read. The
sample buffers are copied out by the functions the screen's capture already has.

**Nothing is scaled or resampled on the way in.** The pictures come at the size the
session's default preset gives the camera and the sound in the microphone's own rate and
channels, and the daemon is told both. The encoder fits the picture: for a camera its
filter is `scale` with the shape kept and `pad`, so a 4:3 camera in a 16:9 share has black
at its sides, on every platform; a 640 by 480 picture into 1280 by 720 was run through
x264 and VA-API here. The Opus encoder starts with the first buffer of sound, which says
the rate and channels it resamples from (`Sound::start_pcm_from`). A pacer thread writes
the newest picture at the agreed rate, starts the encoder again if the camera changes its
size, and ends the share with `error` when the camera has sent nothing for six seconds,
which is how an unplugged camera is noticed.

**Camera and Microphone are asked for in a dialog, and need no restart.** `Ligature.app`'s
`Info.plist` gained `NSCameraUsageDescription` and `NSMicrophoneUsageDescription`, without
which macOS ends a process that opens either, and a bundle signed with a Developer ID gets
the two entitlements the hardened runtime wants. The first share of a camera shows macOS's
dialog and waits up to 30 seconds for the answer: allowed, it goes on; refused, it is
refused for `consent`. A refused or missing microphone leaves the camera silent.
`Permission` gained `Camera` and `Microphone`, which `lpctl permissions` lists and opens
and `reset` forgets, but they are apart from the three a grant of which takes a new
process (`Permission::RESTARTING`): the daemon neither waits for them nor restarts on
them, and the report says "it applies at once" where the others say the daemon restarts.
A camera refused in System Settings is not advertised; one never asked for is.

**No presenter.** The Mac's camera family is attached with a capture and a viewer and no
presenter, so it never says `presents`: a camera offered to macOS's programs is a Core
Media I/O extension, which macOS loads only from an application signed with a Developer ID
and notarised by Apple. That is Apple's rule for the Mac, not a limit of the protocol, and
the platform notes say so.

**What was checked.** The crate and the daemon type-check and pass clippy for
`aarch64-apple-darwin` on Linux, which proves the calls exist with these types and nothing
about how they behave. None of it has run on a Mac: the listing, the dialogs, the pictures,
the microphone's format and the indicator are for the owner's run.
