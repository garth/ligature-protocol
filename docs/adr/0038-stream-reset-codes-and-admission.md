# 0038. Stream reset codes, and streams held until the peer is admitted

**Status:** Accepted
**Date:** 2026-10-23
**Extends:** [ADR 0036](0036-stream-framing-and-message-typing.md) and
[ADR 0019](0019-pairwise-least-exposure-encryption.md)

## Context

ADR 0036 gives every stream a header naming its purpose and one application error code,
`1`, for a purpose the receiver does not understand. Nothing else about how a stream is
refused was written down, and the reference implementation grew three more uses of a reset
without a home for them: a message from a device that is not the person's is answered by
resetting the stream, a frame or header that cannot be decoded likewise, and the `files`
family defined its own codes `2` (the user cancelled) and `3` (failure) for its data
streams. The security audit of 2026-10-23 then added a fourth: a peer the daemon has not
recognised may open nothing but the pairing offer, its other streams wait for the daemon's
verdict, and they are reset if it is dropped (LP-121). That reset took code `2`, the same
number as the files cancel, so a file sender whose data stream was refused for admission
recorded a cancel. Two uses of one number across the transport and a family is exactly
what a code space prevents, and the admission hold is a wire-visible behaviour (a peer sees
its streams answered late or reset) that ADR 0019's "nothing before recognition" implies
but no chapter states.

The `pairing` purpose, which chapter 02 has used since milestone 1 and which the admission
rule singles out, was also missing from the list of purposes ADR 0036 gives.

## Decision

1. **Transport codes are 1 to 15, and apply to any stream.** A receiver resets a stream
   with: `1` when it does not understand the purpose (ADR 0036, unchanged); `2` when the
   peer that opened it has not been admitted past the pairing offer (decision 3); `3` when
   the first message on it is one the receiver will not act on from this peer, such as a
   capability message from a device that is not the person's; `4` when a frame or the
   header cannot be decoded. The connection is unaffected by any of them. Codes `5` to
   `15` are reserved for the transport.
2. **A capability family's own codes start at 16.** A family that resets its streams for
   its own reasons numbers them from `16` in its section of chapter 05 and its schema. The
   `files` family's codes become `16` (the user cancelled) and `17` (failure); no other
   family defines any today. A code at or above `16` on a stream whose family defines none
   is treated as `17`.
3. **Streams wait for admission.** After the handshake a receiver dispatches only the
   `pairing` purpose until it has recognised the peer (chapter 03 section 4). Every other
   stream the peer opens is held, unanswered, in a small bounded queue, and is dispatched
   when the peer is admitted or reset with `2` when the peer is dropped, when it is kept
   for the pairing offer only and the queue is full, or when it opens more than the queue
   holds. A peer kept for the pairing ceremony learns nothing from the streams it opened,
   and a newcomer whose manifest and capability streams arrive the instant it joins has
   them dispatched once the introducer has persisted the grant.
4. **`pairing` is a purpose.** The header's purpose is `gossip`, `manifest`, `pairing`, or a
   capability type; chapter 02 defines the pairing stream's messages.

## Consequences

`lp-core::frame` carries the four transport codes and `lp-core::files` the family's two,
renumbered; a file sender that sees `16` or `17` records a cancel or a failure as before,
and one that sees `2` or `3` records the transport's refusal. The stream loop in
`lp-transport::manager` holds and releases streams as decision 3 says, and resets a
message it will not act on with `3` where it used `1`. Chapter 04 section 3.1 lists the
codes and the `pairing` purpose and section 3.2 the admission hold; chapter 05 section 3d
renumbers the files codes; `schemas/message.schema.json` lists `pairing` and the codes. A
device of this version meeting one of the previous alpha reads its files codes `2` and `3`
as the transport's `2` and `3`, which it records as a refusal rather than a cancel; both
end the file the same way, and the next release carries only the new numbers.

The milestone 7 and 8 plans had reserved 0038 and 0039 for their own decisions; those
become 0040 (a binding standing in for the principal) and 0041 (datagrams).
