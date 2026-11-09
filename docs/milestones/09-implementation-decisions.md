# Milestone 9 implementation decisions

Choices that are not part of the protocol but that a cast, a window share and a share's
sound need settled. Each is recorded under the ticket that made it; the protocol-level
choices are in [09-cast-and-sound.md](09-cast-and-sound.md) section 3.

## LP-199: the daemon core, the offer path, windows, sound and the receivers

**Sound travels on the capture's channel.** A platform's capture already hands the sync one
channel of `Captured` items, frames and the end; sound joins it as `Captured::Audio`, a packet
with its capture time, rather than a second channel or a second trait. A capture that has no
sound to give never sends one, and the platforms of milestone 8 compile unchanged. The sync
forwards each packet as an `audio` message with its own sequence, stamping a packet whose
capture gave it no time with the sync's clock, as it does a frame. When several items have
piled up while the last frame was written, the packets all go first and then the frames,
brought up to date as before: sound never waits for a picture and is never dropped for flow
control. On the viewer the packets come out of the same `Viewed` channel as the frames; one
the viewer cannot take at once is lost rather than queued, since its clock places the next.

**A share's settings carry the source's kind and the sound agreed.** `Settings` gains `kind`
(a display or a window) and `audio` (the format picked, or none); `display` stays the
source's id for either kind, so the backends read it as before. A window has no input
region, so the sync sets none for the viewer and a window share is never driven. The
`Started` type now names the source as `display` or `window`, exactly one, as the schema
does, and `Started::source_id` gives whichever it is to the programs that print it.

**An offer waits in the sync for a decision.** An incoming stream that begins with `offer`
is refused `no-decoder` when nothing offered is decoded here, `declined` when nothing on this
device shows casts, and `busy` when the same caster has an offer waiting; otherwise the sync
records it as pending, tells the device's receiver hook (`set_on_offer`) and waits a minute
for `accept` or `decline`. `accept` answers `start` with what this device decodes, the
picture no larger than the source or the viewer's bounds, the rate no faster than either
side's, and the sound this device decodes of what was offered when sound was wanted; the
caster's `started` must name the source it offered. The handle `accept` returns is the same
`Watching` a watch gives, so `lpview` and the phone show a cast exactly as they show a watch.
`receives` is advertised when the device views and a receiver hook is set, within the
manifest's poll.

**The desktop's receiver is `lpview`.** When an offer arrives the daemon starts `lpview
--receive <caster>` from beside its own executable (`~/.local/bin`, or `target/debug` in a
checkout), passing its data and runtime directories, and `lpview` takes the offer over the
control socket (`screen.accept`); no `lpview` beside the daemon means no `receives`. A cast
nobody takes within a minute is declined. Closing `lpview` ends the share as a watch's does.

**Casting over the control socket.** `screen.cast` holds its connection until the cast ends,
sending `screen.started` once the receiver took it and the response with the reason at the
end, as `screen.start` does, but the frames stay in the daemon: a client that goes away does
not end a cast, which `lpctl screen stop` or the caster's indicator does. `screen.accept`
streams a cast's frames and packets as `screen.start` does, `screen.decline` leaves one, and
`screen.status` lists casts and received casts with their sound and the offers waiting.
`lpctl screen start --audio-out FILE` writes the packets as records of a 4-byte length, an
8-byte capture time and the packet, so a share's sound can be checked without a player.
`screen.displays` without a key answers with this device's own displays and windows, which
`lpctl screen displays` with no device prints, since a cast names its window by an id only
the casting device knows.

**The fake capture does windows and sound.** `FakeCapture::with_windows` lists windows it
shares by id, `set_audio_encodes` says what sound it encodes, `emit_audio` delivers a packet,
and the generating capture of `LIGATURE_FAKE_SCREEN=1` encodes stereo Opus and sends fifty
synthetic packets a second when a share asked for sound; the fake-screen daemon decodes Opus
too, so a cast with sound runs end to end on one machine. A stopped share is forgotten by the
fake, so `running()` says what runs now.

## LP-200: Linux sound, the window picker and `lpview`'s playback

**Sound is another ffmpeg, through Ogg.** The default output's monitor (`pactl
get-default-sink` plus `.monitor`, PipeWire's PulseAudio interface) is captured by ffmpeg's
`pulse` input and encoded by its `libopus` in 20 ms packets at 64 kbit/s a channel, the
`lowdelay` application; ffmpeg writes bare Opus packets in no form the daemon could cut, so
it writes Ogg with a page a packet (`-page_duration 20000`, flushed) and the daemon takes
the packets out of the pages (`screen::ogg`, a reader and a writer of some hundred lines,
RFC 3533 and RFC 7845). The packets carry no capture time of their own: the sync stamps
them on arrival, as it does a frame the encoder did not stamp, so sound and picture sit on
one clock at the sharer and the run measures what the pipelines add. A machine without
`libopus`, the `pulse` input or a default output advertises no `audioEncodes`, and a sound
capture that fails or ends leaves the share silent rather than refused or ended. The
viewer side advertises `audioDecodes` when ffmpeg decodes Opus.

**`lpview` plays through ffmpeg too.** The proposal was `cpal`; ffmpeg already decodes the
picture in `lpview`, has an output device on both desktops (`pulse` on Linux,
`audiotoolbox` on a Mac) and needs no new crate or build dependency, so a second ffmpeg takes
the packets back in Ogg pages (the `ogg::Writer`, with the two header pages first) and plays
them on the default output with a 60 ms buffer, `-fflags nobuffer -flags low_delay` on its
input; `--mute` asks for no sound and plays none. Checked here against a null sink with a
one-second tone; the Mac's device is checked in LP-201's run.

**A window is picked, not listed.** The portal's picker is the only way to a window on
Linux (`wlr-screencopy` copies outputs alone, and a toplevel-export protocol is Hyprland's
own), so the Linux capture lists no windows and implements the sync's `pick`: a cast with
`--window` and no id opens the picker on window sources, with no restore token offered or
kept, and the stream's size from the portal (1280 by 720 when it gives none) is what the
offer carries. The picked session waits in a slot under `window-<node>` until the cast's
`start` names it, and a window share never has an input region. GStreamer's `videoscale`
now adds borders and the caps fix the pixel aspect, so a window that changes shape is
letterboxed into the agreed picture rather than stretched, on display shares too.

**The notification says cast.** The sharing notification reads "Casting <source> with
<device>" for a cast and "Sharing …" for a watch, the source a picked window's label or the
display's id, with the same Stop action and the same rules.

**Checked in the nested Hyprland, found VA-API.** The ignored `screen_nested` test now asks
for sound and checks the packets, put back into Ogg, read as Opus by ffprobe: 83 packets in
about two seconds beside 30 frames, every one read back. The probe found VA-API's H.264
encoder working on this machine, which it had refused at LP-149, so the share came as 4:2:0
and the test takes the sharer's first format rather than assuming x264's 4:4:4. A nested
compositor keeps its window's size until `hyprctl reload` applies the config's monitor line.

**Checked live in the nested Hyprland.** Two fake-screen daemons of one person, paired with
`lpctl pair --yes` on each, the receiver's daemon running with the nested display: `lpctl
screen cast castB` on the caster had the receiver's daemon start `lpview --receive`, whose
window appeared in the nested compositor as `lpview` with the title "castA · fake-1 — click
to drive", the frames and the sound's packets counted the same on both sides' `lpctl screen
status` ("casting" and "receiving", with sound), and closing the window ended the cast with
`viewer` on the caster's `lpctl screen cast`. The fake's synthetic packets are not Opus, so
`lpview`'s player complained of each and played nothing; real packets, from the nested test's
capture, decode. `lpview` now sets its Wayland application id and shows the device's name in
its title when it was given a key, as the daemon gives it.

## LP-201: the Mac's windows and sound

**Windows by id.** `SCShareableContent` lists the windows on screen in the normal layer with
a title, each with its owning application's name and its size in points, which the daemon
offers in pixels at the main display's scale as the `windows` of the `displays` answer; a
`start` or a cast naming one shares it through
`SCContentFilter(desktopIndependentWindow:)` with `scalesToFit`, so the window keeps its
aspect in the agreed picture and the spare area stays blank when it changes shape. A window
share has no input region. The listing needs Screen Recording like the capture, so an
unpermitted Mac lists none.

**Sound from the same stream.** On macOS 13 and later the share's `SCStream` captures the
system's sound too (`capturesAudio`, 48 kHz, the channels agreed, the daemon's own output
excluded) and hands the output object `CMSampleBuffer`s of 32-bit float linear PCM, planar
or interleaved, which `lp-macos` reads through the sample's audio buffer list and hands on
interleaved. The daemon writes them into ffmpeg's stdin (`sound::Sound::start_pcm`: `-f
f32le`, then `libopus` and Ogg as on Linux), and the packets reach the sync as they do from
the Linux capture. `audioEncodes` is advertised when ffmpeg has `libopus` and macOS is 13 or
later; an older Mac shares silently, and the platform notes say so. The bindings gained
`objc2-core-audio-types` and `objc2-core-foundation`, and `objc2-core-media`'s block buffer
and format description features.

**`lpview` plays through AudioToolbox.** The player's ffmpeg writes to `-f audiotoolbox` on
a Mac, the same decoding as on Linux; the daemon starts `lpview --receive` from beside
itself in the bundle's `MacOS` directory as on Linux.

**Type-checked here, run on the Mac.** `cargo clippy --target aarch64-apple-darwin` with the
stand-in C compiler passes for `lp-macos`, `ligatured`, `lpview` and `lpctl`; the sample
extraction, the window filter's scaling and the AudioToolbox device are checked in the
owner's run (LP-203), since nothing links or runs here.

## LP-202: the phone casts, receives a cast, and has sound both ways

**The node's calls.** The app starts a cast with `screen_cast(device, audio)`, which returns
at once: the sync's cast asks the app for the projection through the same
`screen_share_requested` a watch uses, now with the channels of sound wanted, and
`ScreenCastStarted` and `ScreenCastEnded` say what came of it. A cast offered to the phone
reaches the app as `screen_offered` with the caster's name, what is cast and whether sound
comes; `screen_offer_accept` returns the same `ScreenWatch` a watch does, and
`screen_offer_decline` leaves it. The phone always receives, since the app always has
somewhere to show a picture. A watched or received share's packets reach the app through
`screen_audio`, the phone's own through `screen_share_audio`, and `set_screen_share` says
whether the app can send sound.

**The phone's sound needs the recording permission.** Android's playback capture, the only
way to what the phone plays, sits behind `RECORD_AUDIO` as the microphone does, so the
permission is declared, asked for only from the Cast card ("Share the phone's sound too") and
never at first run, and the descriptor's `audioEncodes` follows it and the presence of an
Opus encoder: a phone that has not allowed it shares the picture alone, and the card says
why. The capture takes media, game and unknown usages under the projection; apps that opt
out, and calls, are silent in it, which is Android's rule. Twenty milliseconds of 16-bit
samples at a time go into `MediaCodec`'s Opus encoder (64 kbit/s a channel), and each output
buffer that is not codec configuration is a packet.

**Playing a share's sound.** `WatchSound` configures `MediaCodec`'s Opus decoder with the
identification header of RFC 7845 and the two delays it wants, and writes what it decodes to
an `AudioTrack` in its low-latency mode with a 60 ms buffer; a packet that finds the decoder
busy is lost rather than played late. The watch screen has Mute where a share has sound.

**An offered cast opens where Android allows.** With the app in front the watch screen opens
at once; with the accessibility service on, the app opens itself and wakes the screen, as it
does for a share request; otherwise a notification names the caster and what is cast, a tap
opens it and a swipe declines. An offer is good for 55 seconds on the phone, inside the
node's minute. A received display may be driven as a watched one is, a window never.

**Checked.** The two-node test runs the node end to end: sound with a watch, a desktop's cast
offered, taken with sound and stopped, another declined, and the phone's own cast to a
desktop that receives, with its frames and packets arriving and its end reported. The APK
builds and the JVM tests cover the Opus header, the buffer sizes, the packet sizes and the
choice between opening and notifying. The capture, the decoder and the notification paths on
a real phone are for the run (LP-203), with the owner's go-ahead for the projection.

## LP-203: the run with the real devices, so far

**Run on 2026-11-08 from the Linux machine, the phone locked and nobody at it.** The Linux
daemon was reinstalled from this build through `install.sh` and the phone's app over adb,
both keeping their identities; the Mac stayed on its 0.2.0 build, its lid closed, so it
offered no screen. Found:

- **The older builds are left alone.** `lpctl screen cast` to the Mac and to the phone
  before its update was refused before anything was offered, since neither says `receives`.
  The refusal had named the device by its key under "trust chain"; it now names the device
  and says what it needs (LP-199).
- **A display cast to the phone.** With the app updated, the phone's accessibility service
  woke it and the app took the offer within a fifth of a second; the share ran through the
  portal with its remembered choice and VA-API at 1728 by 1080, about 58 frames a second
  over twenty seconds with none dropped, and `lpctl screen stop` ended it as `sharer` at both
  ends. The phone showed its lock screen, the watch screen behind it, so the picture itself
  was not seen.
- **Sound with it.** With nothing playing on the Linux machine, a cast with sound captured
  the speaker's monitor from the systemd service and sent about 47 packets a second beside
  the picture (280 in six seconds, the first second being the capture's start), and the phone
  created its Opus decoder from the header the app builds and ran until the stop. Whether it
  is heard, and how far from the picture, needs someone listening.
- **One stall.** The first cast's capture went quiet five seconds in, with the portal's "Out
  of buffers", and the watchdog started it again as milestone 8 arranged; the next two casts
  ran without one. It is the portal's own fault of LP-149, not this milestone's pipeline.

**Left for a run with someone at the devices:** the Mac built from this commit (its windows,
its sound, `lpview` receiving and playing through AudioToolbox), a window chosen in the
Linux picker, a cast started on the phone and received in `lpview` here, the phone's own
sound after its recording permission, a cast declined from the phone's notification, the
sound heard on each viewer with the flash-and-beep offset measured, a drive of a cast
display, and clipboard and files during a cast.
