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
- An attacker who has obtained a self-signing key from a compromised introducer.
- A malicious or compromised helper: relay, hole-punch coordinator or discovery service.
- An attacker able to write to the DHT used for discovery (DHT poisoning) or to spoof
  discovery records.
- A compromised device within the set.
- A physical attacker with brief access to an unlocked device.
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
nothing is sent before mutual authentication (chapter 03). A person who publishes their
key reveals what they publish. Open question: whether a device's bare public key, seen by
anyone who connects, is itself an identifier worth hiding behind a per-epoch value.

## 6. Out of scope

A fully compromised operating system on a device, denial of service by a network operator,
and
denial of service by every helper a user has listed at once. Traffic analysis by a helper
(who talks to whom, when, how much) is acknowledged but not defended against in this
version.

## 7. Open questions

- Should a compromised device be able to revoke all others? See chapter 02.
- What a shared device's system-level daemon can see of each account's traffic, and how
  that is minimised.
