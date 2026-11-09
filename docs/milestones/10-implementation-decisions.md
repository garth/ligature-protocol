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
