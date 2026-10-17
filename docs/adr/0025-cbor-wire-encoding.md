# 0025. CBOR as the wire encoding

**Status:** Proposed; item 3 amended by [ADR 0035](0035-byte-exact-signatures-forgiving-decoding.md)
**Date:** 2026-10-17
**Resolves:** the wire-encoding question left open by [ADR 0020](0020-json-schema-and-reverse-dns-names.md)

## Context

ADR 0020 chose JSON Schema as the schema language and left the wire encoding open. The
milestone 1 plan suggested JSON for the proof of concept for readability, with CBOR as the
likely production choice. Starting with one encoding and switching later means two code
paths, two sets of test vectors and a migration, for a protocol whose signed facts depend
on byte-exact encoding. The reasons for CBOR apply from the first commit: keys, signatures
and payload chunks are bytes, and every signed fact needs a deterministic encoding.

## Decision

1. **The wire encoding is CBOR (RFC 8949)**, for the proof of concept and after.
2. **JSON Schema remains the logical schema.** A CBOR message validates against the schema
   after decoding, using this mapping:
   - a schema field of the `bytes` definition (base64url text in JSON) is a CBOR **byte
     string** on the wire;
   - timestamps remain RFC 3339 **text strings** (CBOR tag 0 is permitted but not
     required);
   - public keys remain multibase **text strings**, so identifiers look the same in logs,
     schemas and the wire;
   - everything else maps one to one (maps, arrays, text, integers, floats, booleans,
     null).
3. **Anything that is signed uses RFC 8949 section 4.2 core deterministic encoding**, and
   the signature covers the deterministic encoding of the fact with its own `signature`
   field absent. Implementations must re-encode deterministically before verifying rather
   than trust the bytes as received.
4. **CDDL may be added alongside the JSON Schemas** to pin the byte layout once the message
   set is stable; it is not required for milestone 1.
5. Debugging aids (`lpctl` printing messages as diagnostic notation or JSON) are
   implementation conveniences, not a second wire format.

## Consequences

One encoding from the start; the first conformance vectors are CBOR and stay valid. Byte
fields are a third smaller and need no base64 step. Signed facts have one canonical form.
Developers lose grep-able wire logs and gain a decode step in tooling; the Rust ecosystem
(`ciborium`, `minicbor`) and every other mainstream language cover this. The milestone 1
plan's decision 2 is settled.
