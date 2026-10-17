# 0004. One MLS group per device set for end-to-end encryption

**Status:** Superseded by [ADR 0019](0019-pairwise-least-exposure-encryption.md)
**Date:** 2026-10-17
**Note (2026-10-17):** [ADR 0016](0016-no-required-shared-state.md) removed stored payloads
and offline delivery, two of the three reasons for MLS given below. MLS now rests on relay
fan-out and group-wide post-compromise security. The proof of concept is to be built on QUIC
with cross-signed raw public keys alone; MLS is adopted afterwards if the evidence supports
it.

## Context

Device-to-device traffic must be end-to-end encrypted with forward secrecy and
post-compromise security, and the membership (the device set) changes as devices are added
and revoked. Pairwise sessions (Signal-style) scale poorly with many devices and make
revocation awkward. MLS (RFC 9420) is a standardised group key agreement with efficient
membership changes and mature implementations (OpenMLS). Germ demonstrated the pattern of
atproto identity plus MLS plus an independent transport.

## Decision

Each user's device set is a single MLS group. All peer traffic is protected by that group.
MLS credentials bind to the device keys from ADR 0003. The reference implementation will
use OpenMLS.

## Consequences

*Superseded on 2026-10-17.* Pairwise QUIC/TLS sessions are the end-to-end layer; a group key
would expose data to devices that do not need it. MLS remains a candidate for a later,
capability-scoped multicast key if one is ever shown to be needed.

Original consequences follow.

Adding or revoking a device is an MLS commit. Secrets at rest can be stored as MLS
application messages. LP must solve delivery of MLS handshake messages to offline devices
(the Space is the candidate). Households that include other identities will need either
additional groups or a sub-group design.
