# 0035. Byte-exact signatures in one fact envelope; forgiving decoding

**Status:** Accepted
**Date:** 2026-10-17
**Amends:** item 3 of [ADR 0025](0025-cbor-wire-encoding.md)

## Context

ADR 0025 required signed facts to use RFC 8949 core deterministic encoding so that a
verifier could rebuild the signed bytes from the decoded structure. That makes every
implementation's CBOR output byte-identical by obligation: map key order, integer widths,
float forms, definite lengths. A single difference breaks every signature an implementation
produces, which is the opposite of the forgiveness a multi-implementation protocol needs.
No mainstream CBOR library produces deterministic output by default, so each implementation
would also carry its own canonicaliser.

## Decision

1. **Signatures cover the bytes as transmitted.** A signed fact is never re-encoded. Every
   signed fact travels in one **envelope** (`schemas/envelope.schema.json`):
   `type` (the fact's reverse-DNS type), `payload` (the encoded fact as bytes), `signer`
   (the key that signed) and `signature`. The signature is Ed25519 over the byte
   concatenation `"ligature/fact/1" || type || payload`, so no encoder is involved and a
   signature over one type can never pass as another.
2. **Verify, then decode.** A verifier checks the signature over the payload bytes exactly
   as received, resolves the signer to a trusted chain, then decodes the payload and
   validates it against its schema.
3. **Facts are byte-exact everywhere.** Stored and forwarded as received. The fact
   identifier is the SHA-256 of the payload bytes, which is therefore the same on every
   device.
4. **Decoders are forgiving.** Any valid CBOR is accepted: any map key order, any integer
   width, indefinite lengths. Unknown fields are ignored, and schemas no longer forbid
   additional properties. Encoders are encouraged to emit preferred serialisation
   (shortest forms, definite lengths) and never require it from a peer.
5. **Every signature in the protocol is over a byte concatenation**, never over an
   encoding: a subordinate key's signature by the master key is over
   `"ligature/subkey/1" || purpose || key bytes`; a group acceptance by a member's
   user-signing key is over `"ligature/accept/1" || group key bytes || member key bytes`.
6. **One fact, one envelope.** A role signature is its own fact (`role`), as is a
   revocation (`revocation`); device records, person facts, group documents and grants
   carry no inline signature fields. The store links facts by subject key.

## Consequences

Implementations may use any CBOR library as it comes; no canonicaliser exists anywhere.
Interoperability depends only on producing valid CBOR that matches the schemas. A fact
cannot be "tidied up" and keep its signature, which is correct. Schemas lose their inline
signature fields and their `additionalProperties: false`, and gain `envelope`, `role` and
`revocation`. Chapter 06's signed-facts table names the signer of each envelope.
