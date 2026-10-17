# 0002. Use AT Protocol DIDs as the user identity root

**Status:** Superseded by [ADR 0017](../0017-master-key-identity-and-proximity-ceremony.md)
**Date:** 2026-10-17

## Context

LP needs a stable, user-controlled identifier that is independent of any device and of any
single server, supports key rotation and recovery, and already has an ecosystem of
resolvers and hosting. Options considered: a bespoke key-based identity, Matrix user IDs,
OpenID Connect, and AT Protocol DIDs.

## Decision

A user is identified by an AT Protocol DID, supporting `did:plc` and `did:web`, with a
DNS handle for human-readable naming. `did:plc` rotation keys are the recovery root.

## Consequences

*Superseded on 2026-10-17.* A per-person master key is now the root of trust and a DID is
an optional anchor. With no shared state (ADR 0016) and rebuild instead of recovery (ADR
0018), the DID's rotation and naming roles were no longer required, and it imposed a
server dependency on first run.

Original consequences follow.

LP inherits atproto's resolution infrastructure, handle verification and key rotation
semantics, and can reuse the same identity for Spaces (ADR 0006) and Lexicons (ADR 0007).
`did:plc` depends on the PLC directory, a centralised but mirrorable service; `did:web`
avoids that but makes recovery harder. LP identity is coupled to atproto's evolution.
