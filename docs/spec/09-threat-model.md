# 09. Threat Model

**Status:** Draft.

## 1. Intent

This chapter records who LP defends against, what it protects, and what it explicitly does
not. It should be read alongside every other chapter; design choices elsewhere cite it.

## 2. Assets

The user's master key, the device set membership, the content of
device-to-device traffic, secrets at rest, and the user's presence and location
information.

## 3. Adversaries

- A passive network observer on the LAN or the internet.
- An active network attacker able to inject or modify traffic.
- A stranger on the local network during pairing, including on a headless device's pairing
  page.
- An attacker who has obtained a self-signing key from a compromised introducer, or a
  group key from a compromised admin device.
- A stranger nearby offering unsolicited shares, if the optional open sharing mode is
  enabled.
- Anyone physically present at a group device (a television, a family tablet) with no
  authentication: a child, a visitor, a burglar.
- A malicious or compromised helper: relay, hole-punch coordinator or discovery service.
- An attacker able to write to the DHT used for discovery (DHT poisoning) or to spoof
  discovery records.
- A compromised device within the set.
- A curious or malicious member of the user's group, including a departed member whose
  access was not fully revoked.
- A compromised shared device, which affects every person bound to it.

## 4. Security goals

Confidentiality and integrity of peer traffic against network attackers and every helper.
No off-device party learns the user's device set, grants or household, since none is
stored anywhere but on the devices. **Least exposure**: no device can decrypt data it was
not the intended recipient of, including hubs and other devices of the same person.
Authenticity of device membership. Forward secrecy per session from TLS 1.3;
post-compromise security from the next session. Open question: whether a leaked
self-signing key should have any remedy short of rebuild (chapter 07).

### 4.0 Groups and shared devices

A group member must not be able to read another member's user-scoped traffic or use
another member's capabilities without a grant. On a shared device, one account's daemon
must not be able to read another account's clipboard or keys; LP relies on the
operating system's account isolation for this and says so (chapter 08). A departed member
is removed from the group record and their standing grants are revoked. Open question:
whether a device-scoped capability invoked by a group member should be visible to the
present user even after approval (an indicator), to limit silent use.

### 4.0a Group-owned devices and sharing

A group-owned device accepts only bindings whose chain reaches a member listed in a
membership document signed by the group key, so a forged or stale document cannot admit an
outsider; versioning stops an old document from re-admitting a removed member. A leaked
group key lets an attacker add members until the group is rebuilt. Shares are accepted per
item by the receiving person and only from known persons and group members by default, so
the worst a member can do is annoy; open mode, if adopted, would let a stranger reach a
prompt and must be time-boxed and off by default. Open question: whether a group-owned
device should rate-limit requests per member to resist a compromised member device.

A group device with nobody signed in acts with the group's authority only, so a person at
the television cannot reach any member's clipboard, notifications, files or telephony; data
reaches the device only by push from a member. A person who signs in to a group device and
walks away exposes their own capabilities for as long as the device keeps the session
present, which is a device-security choice outside this protocol (section 6).

### 4.1 Helpers

Relays, coordinators and discovery services follow the design rule in chapter 04: they are
never trusted with plaintext or identity. A malicious relay sees QUIC packets it cannot
decrypt; it can drop, delay or replay them, all of which QUIC detects,
and it can observe who talks to whom by address and timing. A malicious coordinator can
report wrong addresses, causing a failed hole punch. The worst case for any helper is
denial of service, and clients route around a failing helper to another listed in the
user's records.

### 4.2 Discovery records

Discovery records are signed by the device key and keyed by its public part, so DHT
poisoning or spoofing cannot make a device connect to an impostor: the connection itself
authenticates the peer's raw public key (RFC 7250) against the device set. An attacker can
publish stale or wrong addresses only by compromising the device key, and can suppress
records only by attacking the DHT's availability, which again reduces to denial of service.
Open question: replay of old but validly signed records, and whether records need a
sequence number or expiry.

### 4.3 Relay denial of service

A relay, including the project-run convenience relay, can refuse or throttle traffic. The
mitigation is that users list several relays, that the default personal relay is under the
user's own control (the home hub), and that any relay can be swapped without any protocol
change. Open question: whether relays should require proof of device-set membership to
accept traffic, to resist abuse without becoming identity-aware.

## 5. Privacy goals

Local discovery advertisements should not reveal the user's identity to bystanders, and
nothing is sent before mutual authentication (chapter 03). Content that passes through a
capability, such as the text of a notification or the clipboard, travels only on the
authenticated pairwise session, is held in memory for that session, and is never written
to a store (chapter 05 sections 3b and 3c). A transferred file is written only to the
destination the receiving device chose, by default its download folder, under a name the
receiver reduces to a base name, so a sender can never choose a path on the receiver, and
what a device offers is served only by an id it announced, never listed (chapter 05
section 3d). A large clipboard value is announced rather than pushed, so its bytes reach
only the device on which an application pastes it, and the receiver's copy of a fetched
value lives in a cache it clears when its clipboard moves on (section 3b). Open question: whether a
device's bare public key, seen by anyone who connects, is itself an identifier worth hiding
behind a per-epoch value.

### 4.4 Capabilities that act on a device

Input (chapter 05 section 3g) is the first capability by which a peer acts on a device
rather than reads from or shows on it: a paired device of the same person can move the
pointer and type. That is the design, since a paired device carries the person's
authority (chapter 10 section 5.1), and the family is offered to no one else. A device
that is lost or stolen can therefore drive the person's desktop until it is revoked, which
is why revocation is a gossiped fact that takes effect at the next session start and ends
a live session ([ADR 0039](../adr/0039-revocation-scope-and-live-session-close.md),
chapter 06 section 3), and why the person revokes a lost device first. A driven device
shows nothing while driven beyond the pointer moving, and lifts every key and button it
holds when the drive ends however it ends, so a dropped connection leaves nothing
pressed. What this family cannot defend against is a compromised peer of the same person:
it is the person, as far as the protocol can tell, and the remedy is the rebuild of
chapter 07.

### 4.5 No remote trust

Nothing a stranger can do over the network makes any device recognise them. A `person`
role and a group membership exist only through the in-person ceremony
([ADR 0030](../adr/0030-in-person-verification-mandatory.md)). Impersonation at a distance
is therefore excluded by construction rather than mitigated.

## 6. Out of scope

**Device security.** How a device authenticates its local user, whether and when it locks,
and how it protects its local store are the device's and the user's concern
([ADR 0024](../adr/0024-device-security-out-of-scope.md)). An attacker with an unlocked
device in hand has whatever that device's session has; a user who runs a device without a
lock has accepted that. The protocol assumes a present session is whoever the device says
it is.

Also out of scope: a fully compromised operating system on a device, denial of service by a
network operator, and
denial of service by every helper a user has listed at once. Traffic analysis by a helper
(who talks to whom, when, how much) is acknowledged but not defended against in this
version.

## 7. Open questions

- Should a compromised device be able to revoke all others? See chapter 02.
- What a shared device's system-level daemon can see of each account's traffic, and how
  that is minimised.
