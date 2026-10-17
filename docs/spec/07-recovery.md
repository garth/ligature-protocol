# 07. Recovery

**Status:** Draft.

## 1. Intent

A person must be able to regain control of their identity and device set after losing all
devices, without a central operator. See
[ADR 0009](../adr/0009-social-recovery.md).

## 2. Guardians and threshold shares

The recovery secret (ultimately a rotation key or a key that controls one) is split with
Shamir's Secret Sharing among guardians. A threshold of guardians can reconstruct it. See
[`org.ligatureprotocol.guardian`](../../lexicons/org/ligatureprotocol/guardian.json).
Open question: default threshold and share count.

## 3. In-person verification

Guardians release a share only after verifying the requester in person: a QR code plus a
spoken code, and UWB ranging where hardware allows. Open question: whether a remote
fallback (video call) is ever acceptable.

## 4. Time delay and veto

Recovery completes only after a mandatory delay during which any existing trusted device
can veto. Open question: delay length and whether the delay can be shortened when the
user proves possession of a still-trusted device.

## 5. Guardian liveness and share refresh

Devices periodically check that guardians are reachable and proactively refresh shares so
that stale or lost shares do not silently reduce the effective threshold. Open question:
refresh cadence and how to handle a guardian who leaves.

## 6. Open questions

- How are guardians themselves identified: LP identities, or any contact method?
- What does recovery look like for `did:web` identities?
