# 10. Shared Devices and Groups

**Status:** Draft.

## 1. Intent

Some devices have several user accounts and some people live together. This chapter
defines how a device serves more than one identity, how identities form a group, and how
permission works across the boundary between identities. See
[ADR 0015](../adr/0015-shared-devices-and-groups.md). Two motivating cases:

- *Automatic:* copy on my phone, paste into my account on the shared laptop. No prompt.
- *By permission:* stream a song to the shared laptop's speakers while another family
  member is logged in. That person approves before anything plays.

## 2. Principals on a device

A **device principal** is the device itself: one device key, one system-level daemon, an
owner set, and ownership of the device-scoped capabilities (section 3). A **user-device
binding** is one person's account on that device: its own key, signed by that person's
self-signing key with the `binding` role (chapter 02), and owner of that person's
user-scoped capabilities on the device. Pairing a binding on a shared device also signs the
principal with the `principal` role for that person, in the same ceremony. A personal
phone has one binding and its
owner is the device's owner; a family laptop has one device principal and one binding per
account. Open question: whether the device principal needs its own key at all, or can be
represented by the set of bindings plus an owner record.

## 3. Capability scope

Every capability descriptor carries a scope (see
[`org.ligatureprotocol.capability`](../../schemas/capability.schema.json)):

| Scope | Belongs to | Examples | Who approves cross-identity use |
| --- | --- | --- | --- |
| `user` | A user-device binding | Clipboard, notifications, personal files, telephony on a personal phone | The person whose capability it is |
| `device` | The device principal | Display, speakers and media playback, sensors, physical keyboard and mouse, power | The person present on the device; otherwise the device's policy |

Some families appear in both scopes (media control of a user's own player is `user`;
playback through the device's speakers is `device`). Open question: whether scope is fixed
per capability type or chosen per descriptor by the offering device.

## 4. Groups

A **group** is a set of people (a household, a small team), each identified by their
master key, with admins, a membership document, a list of shared devices it owns and a
policy (see [`org.ligatureprotocol.group`](../../schemas/group.schema.json)). Members must
already know each other (the `person` role, chapter 02). A person joins by invitation from
an admin and acceptance on one of their own devices; their user-signing key (chapter 01)
signs the acceptance. Leaving or removal updates the document and revokes the member's
standing grants (section 6). The membership document is a signed fact held by every
member's devices and gossiped like any other (chapter 06). Open questions: whether a
household needs a membership document at all, or whether pairwise `person` signatures plus
each shared device's owner set are enough; whether a person can be in several groups; what
a child account looks like.

## 5. Grants across people

Within one identity, a person's devices may use that person's user-scoped capabilities
without a prompt (chapter 05). Across identities nothing is implicit:

1. A device requests a capability from a principal that belongs to another identity.
2. If the capability is user-scoped, the owning person is asked on any of their devices.
3. If the capability is device-scoped and someone is **present** on the device (an active,
   unlocked session), that person is asked on that device. If nobody is present, the
   device's policy decides; the default policy denies unless the device's owners have
   allowed the capability for group members.
4. The approver may answer once, or remember the answer as a standing grant with scope and
   expiry (see [`org.ligatureprotocol.grant`](../../schemas/grant.schema.json)).

Requests from outside the group are refused before any prompt. Open questions: how
presence is defined with several seats or a locked screen; whether a device owner can
pre-approve on behalf of a present user; and timeouts for unanswered prompts.

## 6. Encryption across people

Nothing changes at the encryption layer: a cross-person capability runs over a pairwise
QUIC session between the requesting device and the granting principal or binding
(chapter 03), so the data is readable by exactly those two. Revoking the grant or removing
the member simply stops future sessions from being authorised.

## 7. State

There is no group database. The membership document and the shared-device records are
signed facts gossiped among members' devices (chapter 06); a shared device's principal
holds the grants and policy that concern it. Only admins' signatures are accepted on
membership changes. Open question: how a new member's devices first obtain the membership
document when no admin device is reachable (likely: the inviting device hands it over
during acceptance).

## 8. Open questions

- Pairing a shared device: does the first admin pair the device principal, and each person
  then binds their account separately?
- Guest access: time-boxed grants to someone outside the group.
- Audit: should approvals and uses of device-scoped capabilities be logged for the owners?
