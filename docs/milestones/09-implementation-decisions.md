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
