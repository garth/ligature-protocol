# 10. Shared Devices and Groups

**Status:** Draft.

## 1. Intent

Some devices have several user accounts, some devices belong to the whole household, and
some people live together. This chapter defines how a device serves more than one person,
how people form a group, how a group owns devices, and how permission works across the
boundary between people. See [ADR 0015](../adr/0015-shared-devices-and-groups.md),
[ADR 0021](../adr/0021-group-key-and-group-owned-devices.md) and
[ADR 0022](../adr/0022-ad-hoc-sharing-between-people.md). Four motivating cases:

- *Automatic:* copy on my phone, paste into my account on the shared laptop. No prompt.
- *By permission:* stream a song to the shared laptop's speakers while another family
  member is logged in. That person approves before anything plays.
- *Group-owned:* a lamp with no accounts that every family member can switch, after one
  pairing by one admin.
- *Ad hoc:* send a photo from my phone to my partner's phone. They accept, it arrives.

## 2. Principals on a device

A **device principal** is the device itself: one device key, one system-level daemon, an
owner set, and ownership of the device-scoped capabilities (section 3). A **user-device
binding** is one person's account on that device: its own key, signed by that person's
self-signing key with the `binding` role (chapter 02), and owner of that person's
user-scoped capabilities on the device. Pairing a binding on a shared device also signs the
principal with the `principal` role for that person, in the same ceremony. A personal
phone has one binding and its owner is the device's owner; a family laptop has one
principal and one binding per account; a lamp has a principal and no bindings. Open
question: whether a principal with bindings needs its own key, or can be represented by
the set of bindings plus an owner record.

## 3. Capability scope

Every capability descriptor carries a scope (see
[`org.ligatureprotocol.capability`](../../schemas/capability.schema.json)):

| Scope | Belongs to | Examples | Who approves cross-person use |
| --- | --- | --- | --- |
| `user` | A user-device binding | Clipboard, notifications, personal files, sharing, telephony on a personal phone | The person whose capability it is |
| `device` | The device principal | Display, speakers and media playback, sensors, physical keyboard and mouse, power, a lamp's switch | The person present on the device; otherwise the device's policy |

Some families appear in both scopes (media control of a user's own player is `user`;
playback through the device's speakers is `device`). Open question: whether scope is fixed
per capability type or chosen per descriptor by the offering device.

## 4. Groups

A **group** is a set of people (a household, a small team) with a **group key** of its
own, created by the founding admin and held by every admin. The group's public key is its
identity. Its **membership document**, signed by the group key, lists members by master key
with roles, the devices the group owns (section 5) and the group policy (see
[`org.ligatureprotocol.group`](../../schemas/group.schema.json)). Members must already know
each other (the `person` role, chapter 02).

A person joins by invitation: an admin's device hands one of the person's devices the
group's public key and the current document, and the person's user-signing key (chapter
01) signs the acceptance. Leaving or removal is a new version of the document; every
member's devices and every group-owned device learn it by gossip (chapter 06), and the
member's standing grants are revoked. Open questions: whether a person can be in several
groups; what a child account restricts; whether admins' devices should push a new document
to group-owned devices rather than wait for gossip.

## 5. Group-owned devices

A device with no accounts (a lamp, a thermostat, a speaker, a hub) is paired once by an
admin through its pairing page or screen (chapter 02 section 5). The admin signs its
principal with the `principal` role and adds it to the membership document; its owner set
is the group key. From then on:

| Who | Trusts what | Because |
| --- | --- | --- |
| Any member's device | The lamp's principal key | The membership document lists it |
| The lamp | A binding whose chain reaches a listed member's master key | The membership document lists that person |
| The lamp | An admin's binding, for configuration | The admin's master key has the `admin` role in the document |

There is never a present person on a device with no accounts, so group policy decides
every request: the default for a group-owned device allows members the capabilities the
admins have listed for it and denies the rest. Adding a family member makes every
group-owned device available to them with no further pairing. Open question: whether a
group-owned device may also have bindings (a family tablet both owned by the group and
holding accounts), and how the two owner models combine.

## 6. Grants across people

Within one person, their devices may use their user-scoped capabilities without a prompt
(chapter 05). Across people nothing is implicit:

1. A device requests a capability from a principal or binding that belongs to another
   person or to the group.
2. If the capability is user-scoped, the owning person is asked on any of their devices.
3. If the capability is device-scoped and someone is **present** on the device (an active,
   unlocked session), that person is asked on that device. If nobody is present, or the
   device has no accounts, the device's policy decides.
4. The approver may answer once, or remember the answer as a standing grant with scope and
   expiry (see [`org.ligatureprotocol.grant`](../../schemas/grant.schema.json)).

Requests from outside the group and from unknown persons are refused before any prompt,
except under the optional open sharing mode (section 7). Open questions: how presence is
defined with several seats or a locked screen; whether a device owner can pre-approve on
behalf of a present user; and timeouts for unanswered prompts.

## 7. Sharing between people

Sending a photo, link, file or note to another person's device is the `share` capability
(chapter 05 section 3a). The receiving device advertises it only to known persons and
group members, so a sender sees exactly the devices they may send to. Each share is
accepted by the receiving person on the receiving device, as a one-time grant, unless they
have chosen to always accept from that sender. Open question: an opt-in **open mode** in
which anyone nearby may offer a share for a few minutes, which would be the one exception
to "nothing before recognition" in chapter 03 and would need its own threat-model entry.

## 8. Encryption across people

Nothing changes at the encryption layer: a cross-person capability runs over a pairwise
QUIC session between the requesting device and the granting principal or binding
(chapter 03), so the data is readable by exactly those two. Revoking a grant or removing a
member simply stops future sessions from being authorised.

## 9. State

There is no group database. The membership document is a signed fact gossiped among
members' devices and group-owned devices (chapter 06); a device's principal holds the
grants and policy that concern it. Only the group key's signature is accepted on a
membership document, and only a higher version replaces the current one. Open question:
how a new member's devices first obtain the document when no admin device is reachable
(likely: the inviting device hands it over during acceptance, which is the normal case).

## 10. Open questions

- Pairing a shared laptop: does the first admin pair the principal, and each person then
  binds their account separately?
- Guest access: time-boxed group membership with the `guest` role, or grants only?
- Audit: should approvals and uses of device-scoped capabilities be logged for the owners?
- Recovery of a group whose admins have all rebuilt: re-pair every group-owned device, or
  allow a member to claim the group with a new group key?
