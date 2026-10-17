# 0021. A group key, and devices owned by the group

**Status:** Proposed
**Date:** 2026-10-17
**Extends:** [ADR 0015](0015-shared-devices-and-groups.md), [ADR 0017](0017-master-key-identity-and-proximity-ceremony.md)

## Context

A household has devices that belong to nobody in particular and everybody in general: a
lamp, a thermostat, a speaker, a hub. They have no user accounts, so they have no bindings,
and after ADR 0017 a `principal` signature is made by one person for that person's devices.
If Alice pairs the lamp, Bob's phone has no reason to trust it and the lamp has no reason
to accept Bob. Requiring every member to pair every shared device separately is the
pairwise model ADR 0017 rejected, one level up.

ADR 0015 left the group's identity open: a membership document signed by an admin's
binding key, with no key of its own. That makes the document only as trustworthy as one
admin's device, and gives shared devices nothing stable to be owned by.

## Decision

1. **A group has a group key**, created by the founding admin and held by every admin, as
   introducers hold a person's self-signing key. The group's public key is its identity.
2. **The membership document is signed by the group key.** It lists members by master key
   with roles, and lists the devices the group owns by principal key. Any member's device
   accepts a document whose signature verifies against the group key it learned when it
   joined; a newer version replaces an older one.
3. **Joining introduces the group key.** The inviting admin's device hands the new
   member's device the group's public key and the current document in the invitation; the
   member's user-signing key signs the acceptance. From then on the member's devices
   gossip the document like any other fact.
4. **A group-owned device is paired once, by an admin**, through its pairing page or
   screen. The admin signs its principal with the `principal` role as usual and also adds
   it to the membership document. Its owner set is the group key.
5. **Trust follows the document.** A member's devices trust a listed device's principal
   key because the document says so. The device accepts a binding whose chain reaches a
   master key listed in the document, and applies the group policy to it: for a device
   with no accounts there is never a present person, so the policy decides, and the
   default for a device the group owns is to allow members the capabilities the admins
   have listed.

## Consequences

One pairing makes a lamp available to the whole family, and adding a family member makes
every group-owned device available to them without touching the devices. The open question
in chapter 10 about whether a membership document is needed at all is answered: it is the
thing that makes group ownership transitive. Admins hold a second key and must be
introducers for it; losing all admins means the group is rebuilt, consistent with ADR 0018.
A group-owned device must receive the document by gossip to learn new members, so the
admin's device should push it after any change. Membership documents gain a version number
so an old copy can never replace a new one.
