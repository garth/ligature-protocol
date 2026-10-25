# 0044. A share's picture format is negotiated, and names what is sent, never what makes it

**Status:** Accepted (by the owner, 2026-10-25)
**Date:** 2026-10-25

## Context

The screen family (chapter 05 section 3h, milestone 8) carries a display as a video stream.
Its descriptor lists `codecs`, of which `h264` is the only one, and `start` and `started`
name one codec. What the codec name leaves fixed is the colour format: every H.264 stream
in the family is 4:2:0, which keeps a picture's brightness at full resolution and its
colour at half resolution in each direction, since that is what every hardware encoder and
decoder takes. A desktop is mostly text and interface drawn with sharp, coloured edges, and
at 4:2:0 coloured text (syntax highlighting, links, a red underline) blurs at its edges
however many bits the encoder spends.

The remote-desktop systems that care most about text keep the full colour resolution
(4:4:4) where both ends can handle it, and choose per connection:

- Microsoft's RDP graphics pipeline has AVC420 and AVC444 as separate codecs; a client
  says which it takes, and the server falls back to 4:2:0 for one that takes only that.
- Parsec offers 4:4:4 as a client setting where the host's and the client's graphics
  hardware support it, at about half again the bit rate.
- macOS High Performance screen sharing has 4:4:4 between two Apple silicon Macs.
- The Chrome Remote Desktop team added 4:4:4 AV1 decoding to WebRTC for screen sharing.

What the two ends can handle differs by device and changes over time. On the owner's
devices, `libx264` encodes 4:4:4 and ffmpeg's software decoder (which `lpview` uses on
Linux and the Mac) decodes it. VA-API's and VideoToolbox's H.264 encoders do not encode it,
and Android's MediaCodec decoders seldom offer it. Which encoder a device uses is its own
affair: the reference implementation runs ffmpeg as a child process today, and a later one
may use a library, the platform's encoder directly or a codec that is not video at all
(VNC's approach of lossless changed rectangles is one). The protocol has to let devices
agree on what goes on the wire without tying them to how it is made.

## Decision

1. **A format names what goes on the wire.** A *format* is a map of `codec` (a name the
   spec defines, which fixes the bitstream's form: `h264` is H.264 as an Annex B byte
   stream with the parameter sets in every keyframe) and `chroma` (`4:2:0`, the default
   when absent, or `4:4:4`). Further keys (a bit depth, a colour range) may be added later;
   a device that does not know one of a format's keys or values passes over that format.
   A format never names an encoder, a decoder, hardware or software, or a quality setting.
2. **Each device advertises what it can do now.** The descriptor's `params` replace
   `codecs` with `encodes` (the formats this device can produce as a sharer) and
   `decodes` (the formats it can take as a viewer), each most preferred first, so a
   device whose encoders and decoders differ says so. A device lists only formats its
   encoder or decoder accepts at the time, and updates its manifest when that changes, as
   it does for `shares` and `views`.
3. **The viewer offers, the sharer picks.** `start` carries `formats`: the formats the
   viewer will take for this share, in its order of preference. That is its `decodes`,
   or fewer if its user narrowed them (to spend less of a slow link, say). The sharer
   picks the first of them it can encode now and answers `started` with that `format`, or
   `refused` with `no-encoder` when it can encode none of them. The viewer offers rather
   than chooses from the sharer's descriptor, so a choice never rests on a manifest that
   has changed since.
4. **A new codec is a spec entry, not a message.** Adding AV1, VP9 or a codec of changed
   rectangles is a definition of its name in chapter 05 (its bitstream's form, what a
   keyframe is), and devices start listing it. A frame stays a sequence of chunks with
   `key` and `at`, whatever the codec.
5. **Quality within a format stays the sharer's.** The sharer still chooses its bit rate
   or quantiser, and the viewer still bounds only the picture's size and rate.

## Consequences

- Two devices that both handle 4:4:4 (the Linux machine and the Mac through `lpview`) share
  at full colour resolution, and a pair where either does not (anything with the phone)
  falls back to 4:2:0 without anyone choosing.
- An implementation can replace its encoder or decoder with no protocol change; it lists
  what the new one produces or takes.
- `codecs`, and the `codec` of `start` and `started`, are removed before the family's first
  release, so no device has to understand both forms.
- A 4:4:4 stream is larger: Citrix gives about 1.5 to 2 times the data of 4:2:0 for the same
  picture. A viewer on a slow link can leave 4:4:4 out of its offer.
- The schema, chapter 05 section 3h and the milestone 8 plan's decision 3 change with this
  ADR. Like the rest of the family, they stay marked draft while ADR 0008 is Proposed.
- Open question: whether a viewer should also offer a bit-rate ceiling for its link, which
  would be a further key of `start` under this ADR's rule of ignored unknown keys.
