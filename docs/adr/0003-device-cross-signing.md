# 0003. Per-device keys cross-signed by the user identity

**Status:** Proposed; the key location (a DID document or a Space record) is superseded by [ADR
0016](0016-no-required-shared-state.md) and [ADR 0032](0032-no-publication-self-asserted-names.md),
the key set by [ADR 0031](0031-split-keys-from-the-start.md), the revocation rules extended by [ADR
0039](0039-revocation-scope-and-live-session-close.md); the cross-signing decision stands
**Date:** 2026-10-17

## Context

Each device must be individually identifiable and revocable without sharing a single
identity key across devices. Matrix solved the same problem with cross-signing: a master
key signs per-purpose keys, which sign device keys, so a user verifies one chain rather
than every device pair.

## Decision

Every device has its own signing key. The user identity holds a master key that
cross-signs device keys following the Matrix cross-signing model. Devices are added by
QR or short-code approval from an existing trusted device. Any trusted device can revoke
another.

## Consequences

Users verify a device once and the trust propagates. Revocation is straightforward.
A compromised device can revoke honest ones, which the threat model (spec chapter 09) must
address, possibly with a multi-device quorum for revocation. The scheme needs a published
location for the master key: the DID document or a Space record.
