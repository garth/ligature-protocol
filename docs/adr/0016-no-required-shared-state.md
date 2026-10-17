# 0016. No required shared state: local trust stores, gossip, and live capabilities

**Status:** Proposed
**Date:** 2026-10-17
**Supersedes:** [ADR 0006](superseded/0006-atproto-space-shared-state.md)

## Context

ADR 0006 stored the device registry, capability descriptors, grants and settings as records
in an atproto Space on a PDS. Reviewing what that state is for showed that almost none of it
needs to be shared, and some of it should not be persisted at all:

- **Capabilities** describe what a device can do *right now*. A phone that is unplugged no
  longer offers its screen. Persisting descriptors makes them presence data stored in a
  database, which is the wrong shape; the dynamic network should register and unregister
  capabilities as devices come and go.
- **Trust** (which keys belong to the identity) is carried by the device itself: a key
  cross-signed by the user's self-signing key proves membership to any device that trusts
  that identity, with no lookup. Only **revocations** need to travel, since a revoked
  device will not announce its own revocation.
- **Grants** are enforced by the device that issued them, so that device is where they
  live.
- **Group membership** is a signed document any member's device can hand to another.
- The only state that must survive losing every device is the identity itself (the DID
  document and its rotation keys, already off-device) and the guardian list.

A required Space also contradicted the stated principles: it made a PDS a hard dependency
of a protocol that claims no required central server, it exposed registry metadata to the
PDS operator, it added an availability dependency for adding and revoking devices, and it
resolved concurrent writes by last-writer-wins at the server. The Spaces API is also alpha.

Alternatives considered: keep the Space as the source of truth (ADR 0006); replace it
with a shared CRDT replicated to every device over MLS (general-purpose and heavier than
the data needs); or the model below.

## Decision

1. **No shared state is required.** The protocol has no database. The only required
   off-device state is the DID document.
2. **Each device keeps a small local persistent store**: its own keys; the identity's
   public cross-signing keys; the trusted device keys it has learned with their
   cross-signatures; revocations; the grants it has issued; group memberships; and, where
   MLS is used, its MLS state.
3. **Signed facts are gossiped.** When two devices establish a session they exchange the
   signed facts they hold that the other lacks: revocations first, then new device
   bindings and group membership changes. Facts are signed by a device key or by the
   identity's self-signing or user-signing key and are verifiable without any server.
4. **Capabilities are live.** On connect a device sends a manifest of the capabilities it
   currently offers and streams changes for as long as the session lasts. When the session
   ends the capabilities are withdrawn. Nothing about them is persisted by peers.
5. **Spaces and the PDS are not part of the protocol.** An implementation may offer an
   encrypted backup of a device's local store to any storage the user chooses, including a
   PDS, but no other device depends on it.

## Consequences

Two devices on a LAN with no internet have everything. Adding and revoking devices works
offline. The PDS operator learns nothing. Milestone 1 needs no PDS, no Space and no
storage integration.

The cost is the **offline-revocation window**: a device that was offline when a peer was
revoked trusts that peer until it meets any other device. Mitigations: gossip revocations
at the start of every session, use MLS epochs so a removed device cannot follow the group
forward, and require re-verification when a device has seen no trusted peer for a
configurable period. A shared store would not have closed this window either, since the
offline device could not reach it.

MLS's justification changes. With nothing stored and no offline delivery, MLS is no longer
needed to encrypt data at rest or to deliver to absent devices. What remains is fan-out
through relays and group-wide post-compromise security. ADR 0004 stands but is annotated;
the proof of concept should be built on QUIC with cross-signed raw public keys alone, and
MLS adopted with evidence of need rather than by default.

Recovery after losing all devices yields a clean slate: the identity and the guardians are
recovered, a new device is bound, and trust in other devices, grants and settings are
rebuilt as devices are re-paired. Lexicons still define every record and message type;
they no longer imply a repository.
