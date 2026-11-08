# 0047. Another person's revocation needs acquaintance, and never touches the owner's keys

**Status:** Accepted (a security fix, 2026-11-08, LP-185)
**Date:** 2026-11-08
**Supersedes:** the third case of [ADR 0039](0039-revocation-scope-and-live-session-close.md)
decision 2, which honoured a revocation from any person whose role fact covered the subject

## Context

ADR 0039 decision 2 has a revocation count on a device in three cases: the person making it is
the device's owner, the subject is that person's own master key, or a verified `binding` or
`principal` role fact signed by that person covers the subject. The third case lets a person
cut off one of their own devices for everyone who knows them.

The security audit of 2026-11-08 found that the third case proves nothing about whose a key
is. A role fact is one person's signature over a public key, and nothing stops a person from
signing a role over a key that is not theirs. Under ADR 0039 decision 3 a device stores any
fact whose signature verifies against its own signer, from any trusted peer, and the check
looked the revoking person up by master key alone, known or not. So any paired device, an
introducer or not, could make up a person (fresh keys and a self-consistent person fact), sign
that person's role over any key of the set and that person's revocation of it, and gossip the
three facts; every device then treated the subject as revoked, and no fact undoes a
revocation. A lost device could revoke the owner's introducers first, after which the other
devices refused the introducers' gossip and the owner's real revocation of the lost device
never reached them: the one defence chapter 09 section 4.4 relies on, defeated by the device
it is meant to stop.

## Decision

1. **A revocation by a person other than the owner counts only when the owner knows that
   person**: the store holds a `person` role fact over their master key signed by the owner's
   user-signing key, and the owner has not revoked them. A person the device does not know
   was a stranger before their revocation and stays one after it; their revocations are kept,
   as ADR 0039 decision 3 keeps every fact, and are inert.
2. **No other person's revocation touches the owner's own keys.** A subject that a verified
   `binding` or `principal` role fact of the owner's own person covers is revoked only by the
   owner, whatever roles others have signed over it.
3. The first two cases of ADR 0039 decision 2 stand: the owner's own revocations count on the
   owner's devices, and a person's abandonment of their own master key counts everywhere,
   since a made-up person abandoning itself harms nobody.
4. Nothing changes on the wire or in the schemas: the revocation fact, its envelope and the
   gossip are as ADR 0039 has them. The rule is one of interpretation, decided every time a
   revocation is used, as decision 3 of that ADR intends.

## Consequences

- A paired device cannot revoke the rest of the set, or lock the owner's own devices out of
  their person, by inventing a person; it can revoke only what the owner's own keys let it
  (an introducer still can, ADR 0031, the accepted risk 1-E of the audits).
- A known person cutting off one of their devices is still honoured by everyone who knows
  them, which is what the third case was for.
- Until any production path makes a `person` role, no other person's revocation counts on
  any device, which is also what the design intended: cross-person trust begins with the
  ceremony (ADR 0030).
- `lp-identity::trust::is_revoked` implements it, with tests for a person made up by a
  trusted device, a known person's role over the owner's device, and a stranger revoking
  their own device. Chapter 06 section 5 states the rule.
