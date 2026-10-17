# 09. Threat Model

**Status:** Draft.

## 1. Intent

This chapter records who LP defends against, what it protects, and what it explicitly does
not. It should be read alongside every other chapter; design choices elsewhere cite it.

## 2. Assets

The user's identity (DID control), the device set membership, the content of
device-to-device traffic, secrets at rest, and the user's presence and location
information.

## 3. Adversaries

- A passive network observer on the LAN or the internet.
- An active network attacker able to inject or modify traffic.
- A malicious or compromised PDS operator.
- A malicious or compromised helper: relay, hole-punch coordinator or discovery service.
- An attacker able to write to the DHT used for discovery (DHT poisoning) or to spoof
  discovery records.
- A compromised device within the set.
- A malicious guardian, or a coalition of guardians below threshold.
- A physical attacker with brief access to an unlocked device.

## 4. Security goals

Confidentiality and integrity of peer traffic against network attackers, the PDS and every
helper. Authenticity of device membership. Recovery that cannot be completed by fewer than
the threshold of guardians, nor without the time delay. Open question: forward secrecy and
post-compromise security targets beyond what MLS provides by default.

### 4.1 Helpers

Relays, coordinators and discovery services follow the design rule in chapter 04: they are
never trusted with plaintext or identity. A malicious relay sees MLS ciphertext and QUIC
headers; it can drop, delay or replay packets, all of which the transport and MLS detect,
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

Local discovery advertisements should not reveal the user's identity to bystanders. The
PDS should learn as little as possible beyond the existence of records. Open question: what
metadata the PDS necessarily learns and whether that is acceptable.

## 6. Out of scope

A fully compromised operating system on a device, a coalition of guardians at or above
threshold acting together with an attacker, denial of service by a network operator, and
denial of service by every helper a user has listed at once. Traffic analysis by a helper
(who talks to whom, when, how much) is acknowledged but not defended against in this
version.

## 7. Open questions

- Should a compromised device be able to revoke all others? See chapter 02.
- How does the model change when household members join?
