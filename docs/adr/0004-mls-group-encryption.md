# 0004. One MLS group per device set for end-to-end encryption

**Status:** Proposed
**Date:** 2026-10-17

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

Adding or revoking a device is an MLS commit. Secrets at rest can be stored as MLS
application messages. LP must solve delivery of MLS handshake messages to offline devices
(the Space is the candidate). Households that include other identities will need either
additional groups or a sub-group design.
