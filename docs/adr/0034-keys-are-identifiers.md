# 0034. Devices and people are identified by their public keys; names default to the host name

**Status:** Accepted
**Date:** 2026-10-17

## Context

The device schema carried a `deviceId` "derived from the key" without saying how, and
grants, bindings and group documents referred to devices by it. A separate identifier
needs a derivation rule, a second column everywhere, and a mapping back to the key; a
hashed form is no shorter than the 32-byte key it would hide, and a truncated form invites
chosen collisions. People are already referenced by their master key everywhere.

A device also needs a human-readable name. Asking the user to type one at pairing is
friction for no gain when the machine already has a host name.

## Decision

1. **The public key is the identifier.** A device is its principal or binding key in
   multibase form (`z6Mk…`); a person is their master key. There is no `deviceId`. Grants,
   bindings' `principal` reference, group documents' owned devices and every other
   reference carry the key string, validated by the shared `publicKey` definition.
2. **Short fingerprints are presentation only.** A terminal or log may show the first
   characters after the `z6Mk` prefix, or a word-list rendering, to tell devices apart.
   Such a fingerprint is never stored, compared or sent.
3. **A device's display name defaults to its host name** as the operating system reports
   it, without any domain suffix, and the user may override it at pairing or later. The
   name is part of the signed device record, so an override is a new version of the record
   signed by the owner. For a person, the display name is self-chosen in the person fact
   (ADR 0032).

## Consequences

One rendering for every identity in the protocol, verifiable on sight: a grant naming
`z6Mk…` can be checked against that key without a lookup. Four schemas lose a field and
`lp-core` loses `DeviceId` in favour of a key newtype. Identifiers are 48 characters, which
is why fingerprints exist for display. First pairing shows a sensible name with no typing.
