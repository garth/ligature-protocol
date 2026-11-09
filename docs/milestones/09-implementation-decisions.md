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
