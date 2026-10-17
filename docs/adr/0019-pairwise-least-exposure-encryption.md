# 0019. Pairwise session encryption with least exposure

**Status:** Proposed
**Date:** 2026-10-17
**Supersedes:** [ADR 0004](0004-mls-group-encryption.md)

## Context

ADR 0004 chose one MLS group per device set, justified by encrypting stored payloads,
delivering to offline devices and group-wide post-compromise security. ADR 0016 removed
the first two. A requirement then surfaced that settles the rest: communication must be
private to the group, but data must not be needlessly decryptable by devices that do not
need it. A group key does the opposite: every message is readable by every device in the
group, including a hub that only forwards it and a tablet that will never show it.

Meanwhile the transport (ADR 0011) already provides what is needed: a QUIC connection is
TLS 1.3, authenticated by the two devices' raw public keys, with ephemeral key exchange.
A relay that forwards packets does not terminate it, so the session is end to end between
exactly the two devices that need the data.

## Decision

1. **Encryption is pairwise and per session.** The QUIC/TLS session between two devices
   is the end-to-end encryption layer. It is authenticated by device keys whose signatures
   chain to the owners' master keys (ADR 0017).
2. **Least exposure.** Data is encrypted only to the device that needs it. Sending the
   same clipboard to three devices is three sessions, not one message readable by all
   three and anything else in the group.
3. **Nothing is exchanged before mutual authentication**: no manifest, no gossip, no
   capability traffic. Bystanders and unauthenticated peers learn nothing about the
   device set or household from a connection attempt beyond a public key.
4. **No group key by default.** If a capability is shown to need genuine multicast
   (one encryption for many recipients through a relay), a key scoped to exactly that
   capability's audience may be introduced by a later ADR. MLS remains the obvious
   candidate for that case.

## Consequences

Milestone 1 has no MLS dependency and no group key management. Forward secrecy comes from
TLS 1.3 ephemeral exchange; post-compromise security comes from the next session. Relays,
hubs and uninvolved devices see ciphertext they cannot open. Fan-out costs bandwidth
linear in recipients, which is acceptable at household scale. `lp-crypto` becomes a crate
for signing and session key material rather than an MLS wrapper. Every spec reference to
MLS state, epochs and key packages is removed.
