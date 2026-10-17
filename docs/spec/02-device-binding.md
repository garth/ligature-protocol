# 02. Device Binding

**Status:** Draft.

## 1. Intent

Devices join a person's device set through one **proximity ceremony**: an existing
device that can introduce verifies a key physically in front of it and signs that key with
a role. One ceremony makes the newcomer available to every device the person owns. See
[ADR 0017](../adr/0017-master-key-identity-and-proximity-ceremony.md) and
[ADR 0003](../adr/0003-device-cross-signing.md).

## 2. Keys on a device

- **Device principal key.** One per physical device, held by the system-level daemon. It
  owns the device-scoped capabilities (chapter 10).
- **Binding key.** One per account per device, held by that account's daemon. It owns the
  account holder's user-scoped capabilities on that device. A personal phone has one
  binding; a family laptop has one per account.
- **Self-signing key**, if this device is an introducer (chapter 01).

## 3. Roles

A signature made in the ceremony carries exactly one role:

| Role | Signed key | Meaning for all my devices |
| --- | --- | --- |
| `binding` | A binding key | This is my account on that device: member of my device set, trusted for my user-scoped capabilities |
| `principal` | A device principal key | This is a device I use: I may request its device-scoped capabilities |
| `person` | Another master key | This is someone I know: their bindings are recognisably theirs and may ask me for grants |

A binding signature also records whether the new binding is an **introducer**, which the
user chooses at pairing time according to their requirements. Open question: whether a
`principal` signature should record owner rights separately from use rights (section 6).

## 4. The ceremony

1. The newcomer displays its key fingerprint and a one-time code, or serves a page that
   does (section 5).
2. An introducer of mine scans or reads it, shows the same short code on both sides, and
   the user confirms they match by voice or by eye.
3. The introducer signs the newcomer's key with the chosen role and sends the signature,
   the self-signing key if the newcomer is to be an introducer, and current gossip
   (chapter 06).
4. Both sides store what they received. Every other device learns of it by gossip.

BLE, NFC or UWB ranging may be used to tighten the proximity check where hardware allows
but are never required. The short code is a six-digit (or two-word) short authentication
string derived from the TLS exporter of the pairing connection, so an interposed attacker
holding two sessions cannot show the same code to both sides
([ADR 0029](../adr/0029-ceremony-code-from-tls-exporter.md)).

```text
  Newcomer                                     Introducer (holds the self-signing key)
  --------                                     --------------------------------------
  generate keys
  show fingerprint + one-time code  -------->  scan the QR, or read the pairing page
  <-------------- QUIC connection, raw public keys presented both ways -------------
  derive code from TLS exporter                derive code from TLS exporter
  display code                                 display code (or ask the user to type it)
                      user confirms the two codes match; mismatch aborts
  <------- role signature(s), self-signing key if newcomer is an introducer, -------
           current gossip
  store                                        store
                      every other device learns of the newcomer by gossip
```

## 5. Headless devices

A device without a screen or camera (a hub, a NAS, a speaker) never relies on hardware
for pairing. It serves a basic web page on the local network, reachable by a URL it
prints at first boot, announces over mDNS, or exposes on a well-known port. The page shows
the fingerprint and code of step 1, and the ceremony proceeds from any trusted device
viewing it. Open question: whether that page should be served only until the first
binding or principal signature arrives, and what replaces it afterwards for adding further
owners.

## 6. Shared devices

A shared device has a principal and zero or more bindings. At pairing the introducer
chooses its **registration**: *personal*, in which case the introducer's person becomes
its owner and may add other owners; or *group*, in which case the introducer must be a
group admin, the owner set is the group key, and the device is added to the membership
document (chapter 10 section 5). A family tablet, a television or a lamp is registered to
the group, never to an individual. Pairing a binding on a shared device signs the binding
as `binding` and the principal as `principal` in one ceremony, since the account's daemon
can attest the principal key over local IPC. A binding on a group device should default to
not being an introducer, but the choice is the user's (chapter 01 section 3). Owners set
the device's policy for unattended requests. Open question: whether a person can hold a
`principal` signature for a device they neither own nor have an account on (a guest using
the speakers), or whether that is always a grant.

```text
  Family laptop
  +------------------------------------------------------+
  | device principal: key P, system daemon               |
  |   owner set {Alice, Bob} (personal) or {group key}   |
  |   device-scoped: display, speakers, physical input   |
  |                                                      |
  |  +----------------------+  +----------------------+  |
  |  | binding Alice@laptop |  | binding Bob@laptop   |  |
  |  | key B1, Alice's      |  | key B2, Bob's        |  |
  |  | account daemon       |  | account daemon       |  |
  |  | user-scoped:         |  | user-scoped:         |  |
  |  | clipboard, notifs    |  | clipboard, notifs    |  |
  |  +----------------------+  +----------------------+  |
  +------------------------------------------------------+
  Alice's self-signing key signs B1 as binding and P as principal, in one ceremony.
  Bob's self-signing key signs B2 as binding and P as principal, in his.
  A lamp is the same picture with no bindings and the group key as owner.
```

## 7. Revocation

Any introducer may revoke a binding, a principal or a known person by signing a
revocation, gossiped to every peer (chapter 06). A revoked device cannot announce its own
revocation, so peers that were offline learn of it from the next trusted device they meet.
Open question: whether revocation of a binding by a non-introducer should be allowed, and
whether a quorum should be required when the device set is large.

## 8. Open questions

- How long a device may go without meeting any trusted peer before treating its trust
  store as stale (chapter 06 section 5).
- Whether a device may hold bindings for two people at once on an operating system with
  no account separation.
