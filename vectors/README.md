# Conformance vectors

Files any implementation can run to show it speaks Ligature Protocol. They are the
conformance definition named in [ADR 0026](../docs/adr/0026-rust-reference-implementation.md),
alongside the specification. Format and layout were settled in milestone 1 decision LP-33
([implementation decisions](../docs/milestones/01-implementation-decisions.md)).

## What a vector asserts

Signatures cover bytes as transmitted and no canonical encoding exists
([ADR 0035](../docs/adr/0035-byte-exact-signatures-forgiving-decoding.md)), so a vector
carries the exact bytes the reference implementation produced and asks an implementation to
act on them. For a signed fact an implementation must:

1. verify `signature` over `"ligature/fact/1" || type || payload` with `signer`;
2. decode `payload` and obtain a structure equal to `input`;
3. compute `factId` as the SHA-256 of `payload`;
4. given the signer's seed from `keys.json`, sign `payload` and obtain `signature` byte for
   byte (Ed25519 is deterministic).

It may also check that its own encoding of `input` equals `payload`. The reference
implementation does; others need not, and nothing in the protocol requires it.

A vector whose `expect.outcome` is `invalid` must fail step 1 (or step 2 where the
description says so). Its `reason` is for people; implementations need not match it.

## Layout

| Path | Contents |
| --- | --- |
| `keys.json` | The test keys: name, 32-byte Ed25519 seed, public key as multibase. **Test material only.** |
| `keys/` | Subordinate-key signatures (`"ligature/subkey/1"`) and multibase rendering |
| `facts/` | Person, role and revocation envelopes with their fact identifiers |
| `ceremony/` | Six-digit codes derived from TLS exporter output ([ADR 0029](../docs/adr/0029-ceremony-code-from-tls-exporter.md)) |
| `gossip/` | Stream header and gossip message frames ([ADR 0036](../docs/adr/0036-stream-framing-and-message-typing.md)) |

One JSON file per case, named for what it shows: `role-binding-introducer.json`,
`role-wrong-type-rejected.json`. Negative cases matter as much as positive ones.

## File format

Every file carries `"format": "ligature-vector/1"` and a `description`. Byte fields
(`payload`, `signature`, `factId`, `seed`, `exporter`) are lowercase hex; vectors are not
records, and hex is what a person reads in a debugger. Inside `input`, bytes follow the
schema (base64url) because `input` is the record as JSON. Keys are multibase strings
everywhere. The Rust types in `impl/crates/lp-vectors/src/lib.rs` are the exact shapes.

A fact vector:

```json
{
  "format": "ligature-vector/1",
  "description": "A binding role fact signed by alice-self.",
  "keys": ["alice-self", "laptop-binding"],
  "input": { "...": "the fact as JSON" },
  "type": "org.ligatureprotocol.role",
  "payload": "a3…",
  "signer": "z6Mk…",
  "signature": "…",
  "factId": "…",
  "expect": { "outcome": "valid" }
}
```

## Adding a vector

Vectors are written by a generating test in the reference implementation, never by hand,
and only when `LP_WRITE_VECTORS=1` is set:

```sh
cd impl
LP_WRITE_VECTORS=1 cargo test -p lp-vectors
```

A generating test refuses to overwrite a file that exists. To change a vector, delete it,
regenerate it, and review the diff; a vector that changes means either a bug was fixed or
the protocol changed, and the commit message says which. Every test run, with or without
the variable, loads and checks every file, so a vector can never be committed stale.
