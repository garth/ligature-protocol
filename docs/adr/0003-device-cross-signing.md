# 0003. Per-device keys cross-signed by the user identity

**Status:** Proposed
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
