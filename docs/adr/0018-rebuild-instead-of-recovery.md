# 0018. Rebuild instead of recovery

**Status:** Proposed
**Date:** 2026-10-17
**Supersedes:** [ADR 0009](superseded/0009-social-recovery.md)

## Context

ADR 0009 designed social recovery: Shamir shares held by guardians, in-person
verification, a mandatory delay with veto, liveness checks and share refresh. It assumed
there was something to recover: a DID with its rotation keys and a registry of devices,
grants and settings. ADR 0016 removed the shared state and ADR 0017 removed the DID. What
is left after losing every device is a master key that signed some bindings, a
few standing grants held by other people's devices, and household memberships. Each is
recreated by the same ceremonies that created it the first time.

## Decision

There is no recovery mechanism. A person who loses all their devices, or whose signing
key may have leaked, **rebuilds**: a new device generates a new master key; the devices
they physically hold are re-paired by the proximity ceremony; household members re-verify
the new key in person; standing grants are re-approved as they are next needed. Peers that
still hold the old key treat its bindings as any other unknown person's until told
otherwise, and any device of the person's may gossip a signed statement that the old key
is abandoned.

## Consequences

Chapter 07 shrinks to a page and the `guardian` Lexicon is removed. No background traffic
for liveness or share refresh, no guardians to recruit, no delay to wait out. The cost is
the effort of re-introducing devices and people, which the user has accepted as the price
of statelessness. A compromised key cannot be rotated in place, only abandoned; a person who has published
their key (ADR 0017) can publish the new one in the same place so remote contacts learn of
the rebuild. Guardians and threshold shares remain available as a future extension
for users who want them, and ADR 0009 documents that design.
