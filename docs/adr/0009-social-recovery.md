# 0009. Social recovery with threshold shares, in-person verification, delay and veto

**Status:** Proposed
**Date:** 2026-10-17

## Context

Without a central operator there is no "forgot password" flow. If a user loses every
device they must still be able to recover their DID and device set. Options considered:
printed recovery codes only, a hosted escrow service, and social recovery. Dark Crystal
and similar projects show social recovery is workable; its weaknesses are impersonation of
the user to guardians and guardians who silently drop out.

## Decision

Recovery uses Shamir threshold shares held by guardians. A guardian releases a share only
after in-person verification (QR plus spoken code, UWB where available). Recovery
completes only after a mandatory time delay during which any existing device can veto.
Devices run guardian liveness checks and proactively refresh shares.

## Consequences

No single party can recover the identity alone. Users must recruit and maintain guardians,
which is a real usability cost. Liveness and refresh add background traffic and protocol
surface. The time delay slows legitimate recovery; its length is an open question.
