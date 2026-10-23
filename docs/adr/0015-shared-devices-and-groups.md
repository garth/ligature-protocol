# 0015. Shared multi-user devices, groups, and scoped capabilities

**Status:** Proposed; the MLS device and per-grant groups are superseded by [ADR
0019](0019-pairwise-least-exposure-encryption.md) and the Lexicon changes by [ADR
0020](0020-json-schema-and-reverse-dns-names.md); the scoping, groups and approval decisions stand,
extended by [ADR 0021](0021-group-key-and-group-owned-devices.md) to [ADR
0023](0023-group-devices-sessions-and-authority.md)
**Date:** 2026-10-17

## Context

Real households have devices with several user accounts (a family laptop, a living-room
hub or TV) and several people whose devices should be able to work together. The model so
far assumed one identity owns each device and that all capability traffic stays inside one
identity's device set. That cannot express "copy on my phone, paste on the shared laptop
while I am logged in", nor "stream a song to the laptop while my partner is logged in,
with their consent".

Two things were missing. First, a distinction between capabilities that belong to a
*person's session* on a device (clipboard, notifications, personal files) and
capabilities that belong to the *device itself* (display, speakers and media playback,
sensors, physical input). Second, a notion of a *group* of identities whose devices may
talk to each other, with access gated by permission rather than by membership alone.

Alternatives considered for groups:

- **No groups; pairwise trust only.** Each person explicitly trusts each other person
  (Matrix-style user-signing). Simple, but no place to hang shared-device ownership or
  household policy.
- **One household MLS group containing everyone's devices.** Simple key management, but
  every member's traffic is readable by every other member's devices, which defeats
  user-scoped privacy on shared devices.
- **A group as a first-class principal** with its own identity, records and policy, with
  cross-identity traffic in narrower MLS groups created per grant. More moving parts, but
  it matches how people think about a household and keeps the privacy boundary.

Alternatives considered for shared devices:

- **One device key shared by all accounts.** Cannot distinguish who is acting.
- **One device key per OS account, no device principal.** Clean for user-scoped
  capabilities (one daemon per account, as systemd user services already work), but
  device-scoped capabilities have no owner when nobody is logged in.
- **A device principal plus one user-device binding per account.** The device has its own
  key and system-level daemon that owns device-scoped capabilities; each account's daemon
  owns that person's user-scoped capabilities and is a member of that person's device set.

## Decision

1. **Capabilities are scoped.** Every capability descriptor declares `scope: user` or
   `scope: device`. User-scoped capabilities are offered by a *user-device binding* (one
   person's account on one device) and live in that person's MLS device group.
   Device-scoped capabilities are offered by the *device principal* and are governed by the
   device's owners.
2. **Shared devices have a device principal and per-account bindings.** A shared device
   holds a device key, run by a system-level daemon, and each account on it holds its own
   binding key cross-signed by that account holder's identity exactly as in ADR 0003. A
   personal device is the degenerate case: one binding, and the binding's owner is the
   device's owner.
3. **Groups are first-class.** A group (for example a household) is a set of member
   identities with a membership record, admins, a list of shared devices it owns, and a
   policy. Membership is established by invitation and acceptance between identities, and
   each member's user-signing key (the previously unused half of ADR 0003) signs the
   membership they accept.
4. **Grant defaults follow identity boundaries.**
   - Between a person's own devices, access to their own user-scoped capabilities is
     implicit: copying on their phone and pasting on their account on the laptop needs no
     prompt.
   - Across identities, nothing is implicit. A group member's request to use a
     device-scoped capability on a shared device is approved by the person currently
     present on that device; if nobody is present, by the device's policy as set by its
     owners. A request to use another person's user-scoped capability is approved by that
     person. Approvals can be remembered as standing grants with scope and expiry.
5. **Cross-identity traffic uses narrower MLS groups.** A grant across identities creates
   (or reuses) an MLS group whose members are exactly the requesting devices and the
   granting principal, so a household member never joins another member's device group.

## Consequences

The clipboard case is automatic and the shared-speaker case prompts the person who is
there, which matches expectations. Shared devices need a system-level daemon in addition to
per-account daemons, and the two must agree on who is present (on Linux, via logind). The
Lexicons grow a group record and scope and approval fields. MLS group management becomes
more complex: per-grant groups must be created, rotated when membership changes and torn
down on revocation. Departed members are handled by removing them from the group record
and from every per-grant MLS group they were in. The threat model gains a household-member
adversary and a shared-device compromise that affects everyone bound to it.
