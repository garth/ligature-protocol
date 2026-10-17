# 03. Encryption

**Status:** Draft.

## 1. Intent

LP application messages are end-to-end encrypted so that relays and every other
intermediary see only ciphertext. The planned mechanism is an MLS group (RFC 9420) whose
members are exactly the user's trusted devices. See
[ADR 0004](../adr/0004-mls-group-encryption.md) and its note: since
[ADR 0016](../adr/0016-no-required-shared-state.md) removed stored payloads and offline
delivery, MLS rests on relay fan-out and group-wide post-compromise security, and the
proof of concept is built on QUIC with cross-signed raw public keys alone. Open question:
whether MLS is adopted at all, to be decided on evidence from the proof of concept.

## 1a. Layering: why QUIC/TLS and MLS both exist

Two encryption layers are present on every peer connection:

- **Underneath: QUIC/TLS 1.3 authenticated by device keys** (chapter 04). It protects a
  single hop, authenticates the two endpoints to each other using raw public keys, and
  gives the transport its integrity and migration properties. It is defence in depth.
- **On top: MLS, end to end for the device group.** It protects content from relays, DHT
  nodes and any other intermediary, none of which terminate MLS. It also gives forward
  secrecy and post-compromise security across the whole device set rather than per
  connection, and lets one message fan out to several devices through a relay.

A relayed connection terminates QUIC at the relay only if the relay is a QUIC proxy; even
then the relay sees MLS ciphertext. A direct connection has both layers between the same
two devices, which is redundant for confidentiality but keeps the model uniform: the
application never needs to know whether a path is direct or relayed. Open question:
whether the QUIC layer should use the MLS exporter to bind the two layers together
(channel binding) so that a QUIC peer must also be an MLS group member.

## 2. Group lifecycle

A device set has one MLS group. The first device creates it; each added device is
committed into the group by an existing member; a revoked device is removed. On a shared
device, each user-device binding is a member of its own person's group; the device
principal is in none of them. Cross-identity grants create narrower MLS groups (chapter 10
section 6); another person's device never joins this one.

## 3. Credentials

MLS leaf credentials bind the MLS key to the device signing key from chapter 02, so that
group membership can be verified against the cross-signing chain. Open question: whether to
use basic credentials carrying the device key or a custom credential type.

## 4. Key packages and delivery

Key packages for new devices and MLS handshake messages are delivered directly over the
transport when devices meet, as part of gossip (chapter 06). There is no store-and-forward
service. Open question: how a device that is rarely online with any peer catches up on
commits, and whether that is the point at which MLS stops being worth it.

## 5. Application messages

Capability traffic (chapter 05) is carried as MLS application messages or, for bulk data,
as streams keyed from the MLS exporter secret. Open question: the threshold and mechanism
for bulk transfer.

## 6. Secrets at rest

The protocol stores nothing off-device. Each device protects its own local store with
platform facilities (chapter 08). An optional backup (chapter 06 section 6) is encrypted to
a key only the user holds.

## 7. Open questions

- Which ciphersuite(s) are mandatory to implement?
- How do devices that are offline for a long time catch up on epochs?
- How many cross-identity MLS groups a busy household ends up with, and whether that is
  acceptable (chapter 10).
