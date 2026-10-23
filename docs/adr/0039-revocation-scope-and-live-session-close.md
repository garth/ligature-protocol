# 0039. Whose revocation counts, and a gossiped revocation ending the live session

**Status:** Accepted
**Date:** 2026-10-23
**Extends:** [ADR 0003](0003-device-cross-signing.md),
[ADR 0016](0016-no-required-shared-state.md) and [ADR 0018](0018-rebuild-instead-of-recovery.md)

## Context

Chapter 02 section 7 and chapter 06 section 3 say that a revocation or a key abandonment is
signed by "an introducer's self-signing key" and verified against "the relevant master key",
and leave the rest to the reader: which master key is relevant, on whose devices the
revocation then counts, and what a device that already holds a session with the revoked
peer does. The security audit of 2026-10-23 found that the reference implementation honoured
any well-formed revocation on the strength of its subject alone (finding 1-A, fixed under
LP-120) and that a device learning of a revocation by gossip kept its live session with the
revoked peer (finding 1-C, fixed under LP-122). The fixes settled both questions in the code
but nowhere else, and the fix for 1-A rests on a property of the store that chapter 06
section 3 contradicts: the chapter says a device accepts a fact only if its signature
verifies against a key it already trusts, while the store keeps every fact whose signature
verifies against its own signer and decides, when the fact is used, whether that signer is
one to listen to. Chapter 07 section 3 also lets "any introducer of the new key" abandon an
old master key, which no device of another person could honour under the fixed rule, and
leaves open whether the old key should sign the abandonment itself.

The facts in play: a person fact carries the self-signing key under the master key's
signature (ADR 0032), so verifying a revocation against the person fact of the person it
names binds the signer to that person. A person's self-signing key is held by their
introducer devices (ADR 0031), so only an introducer can sign one.

## Decision

1. **A revocation names the person making it and is verified against that person.** The
   revocation payload (`schemas/revocation.schema.json`) names a `person` (a master key) and
   a `subject`; the envelope is signed by that person's self-signing key, and a verifier
   checks it against the newest verified person fact of the named person. A revocation
   that names a person the store has no fact for, or that does not verify against that
   person, is kept but inert.
2. **A revocation counts on a device when one of three holds.** Seen from a device whose
   person is the owner, a verified revocation of a subject counts when:
   - the person it names is the owner (a person revoking anything on their own devices,
     including a known person's master key or a device of theirs);
   - the subject is the master key of the person it names (an abandonment, decision 4);
   - a verified `binding` or `principal` role fact signed by the person it names covers the
     subject (a person revoking one of their own devices, honoured by everyone who knows
     them).
   Otherwise the revocation changes nothing on that device: one person's revocation of
   another person's device, and a stranger's revocation of anything, are inert. Revocation
   is not undone by any fact: a later role fact over a revoked key does not restore it, and
   a revoked person stays revoked.
3. **Store, then decide.** A device stores any fact whose envelope signature verifies
   against the signer it carries and whose payload decodes (chapter 06 section 3). Whether
   the fact counts is decided every time it is used, against the chain of facts the store
   holds at that moment: a role fact counts when its signer is a subordinate key of a verified
   person fact, a revocation when decision 2 says so. This is what lets the facts of a gossip
   exchange arrive in any order and a revocation take effect the moment the person fact that
   validates it is present, and what keeps a fact a stranger signed from mattering however
   well-formed it is. A fact that fails its own signature is quarantined and never used.
4. **An abandonment is a revocation of the person's own master key, signed by that person.**
   It verifies like any revocation, so it needs the abandoned person's self-signing key,
   which an introducer still holds after a loss that spared it. A person who has lost every
   introducer cannot abandon the old key: they rebuild under a new one (chapter 07 section 2),
   household members re-verify them and may revoke the old `person` role on their own devices
   (decision 2, first case), and devices that never hear either treat the old key's bindings
   as an unknown person's. No third party can abandon a key on a person's behalf.
5. **A revocation ends the live session.** A device that learns a revocation, by signing it,
   by gossip or at the end of a pairing ceremony, re-decides the recognition of every
   connected peer at once and closes the session of any peer no longer trusted. A revoked
   device is also refused on its next connection (chapter 04 section 3.2); the two together
   are what chapter 06 section 5 means by "a revoked device gains nothing from sessions it
   held".

## Consequences

The reference implementation already behaves this way: `lp_identity::trust::is_revoked`
applies decision 2, `TrustStore::insert` and `gossip::apply` decision 3, `facts::revocation_fact`
decision 4, and `Daemon::enforce_trust` decision 5 after every gossip exchange, revocation
and ceremony. Chapter 02 section 7, chapter 04 section 3.2, chapter 06 sections 3 and 5 and
chapter 07 section 3 are reworded to say the same; the schema's description names the rule.
The open question of chapter 02 section 7 about a non-introducer revoking a binding is
closed by decision 1 (a non-introducer has no self-signing key to sign with); the question
of a quorum for large device sets stays open. The open question of chapter 07 section 3 is
closed by decision 4.

What decision 2 does not give: a person cannot revoke another person's device for everyone,
and a household cannot cut off a member's device without that member. Both are by design
under ADR 0003 (each person's device set is theirs). The cost of decision 4 is that a person
whose introducers are all lost or compromised has no abandonment to gossip; ADR 0018 accepts
that cost, since re-verification in person replaces it.
