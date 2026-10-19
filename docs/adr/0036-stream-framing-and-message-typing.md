# 0036. Stream framing, message typing and versioning on the wire

**Status:** Accepted
**Date:** 2026-10-18
**Extends:** [ADR 0011](0011-quic-application-transport.md),
[ADR 0028](0028-quinn-rustls-raw-public-keys.md) and
[ADR 0035](0035-byte-exact-signatures-forgiving-decoding.md)

## Context

ADR 0011 gives every device pair one QUIC connection and every capability its own stream,
ADR 0025 makes CBOR the wire encoding and ADR 0035 fixes the envelope for signed facts.
Three things were still unsaid, and two implementations could not interoperate without
them: how one CBOR message is told from the next on a stream, how a stream says what it is
for (QUIC streams have numbers, not names, and both peers open them), and how a peer learns
which protocol version it is talking to. A fourth point follows from the envelope: its
`type` field names the payload, so an envelope sent bare on a stream would collide with any
rule that the top-level `type` names the message.

## Decision

1. **Frames.** A message on a QUIC stream is a frame: a 4-byte big-endian length followed
   by exactly one CBOR map. A message is at most 1 MiB unless the specification of the
   stream's capability says otherwise; a receiver closes a stream that announces a larger
   length before decoding anything. A CBOR sequence (RFC 8742) was rejected because a
   reader cannot bound a message without parsing it and a single corrupt byte desynchronises
   the stream for good.
2. **Every frame is a typed message.** The map carries a `type` field with the message's
   reverse-DNS schema name, as records already do. Decoders are forgiving as in ADR 0035:
   unknown fields are ignored.
3. **Envelopes never travel bare.** A signed fact is always a field of a message. On the
   gossip stream the messages are `gossip.have` (the fact identifiers a side holds),
   `gossip.want` (the identifiers it lacks) and `gossip.fact` (one envelope). The envelope's
   payload bytes are carried unchanged, so nothing in ADR 0035 is affected.
4. **A stream header names the stream.** Every stream is bidirectional, and the first frame
   on it is `org.ligatureprotocol.stream` with a `purpose`: `gossip`, `manifest`, or a
   capability type such as `org.ligatureprotocol.capability.clipboard`. Whichever peer
   needs a stream opens it and sends the header; the receiver dispatches on it. The dialer
   opens the gossip stream first, which is how "revocations first" is realised. A header
   naming a purpose the receiver does not understand is answered by closing that stream with
   the application error code `1` (unknown purpose); the connection is unaffected. Two
   streams with the same purpose open at once are both accepted.
5. **The version is the ALPN token.** The TLS handshake carries `ligature/1`; peers that
   share no token never complete a connection and never parse each other's frames. No
   message carries a version field. Within a version, evolution is additive: unknown fields
   are ignored, unknown purposes are refused per stream, and capability types evolve by
   name.

## Consequences

One frame codec serves every stream and, in the reference implementation, the local control
channel too. `schemas/message.schema.json` defines the stream header and the gossip
messages; manifest and capability messages follow in their own schemas. Chapter 04 section
3.1 gains the header and the ALPN token and loses its open question on stream identifiers;
chapter 06 names the gossip messages. The 1 MiB default means the `files` family must
define chunking in its own specification, which was always going to be true.
