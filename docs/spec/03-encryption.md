# 03. Session Encryption and Exposure

**Status:** Draft.

## 1. Intent

Communication is private to the people and devices involved, and no device can decrypt
data it does not need. The QUIC/TLS 1.3 session between two devices (chapter 04) is the
end-to-end encryption layer; there is no group key. See
[ADR 0019](../adr/0019-pairwise-least-exposure-encryption.md).

## 2. The session is the envelope

Every QUIC connection is authenticated by the two devices' raw public keys (RFC 7250),
whose signatures chain to their owners' master keys (chapters 01 and 02). Key exchange is
ephemeral, so a captured session cannot be read later even if a device key leaks. A relay
forwards packets and never terminates TLS, so the session is end to end whether the path
is direct or relayed. Open question: whether to mandate TLS key updates at an interval for
long-lived sessions.

## 3. Least exposure

Data is encrypted only to the device that needs it. A clipboard change meant for three of
my devices is three sessions, not one message readable by all three and by every other
device that happens to hold a shared key. Hubs, relays and uninvolved devices see only
ciphertext. Open question: whether any capability needs genuine multicast badly enough to
justify a key scoped to exactly that capability's audience; if so, it would be introduced
by its own ADR.

```text
  phone =========== session A ===========> laptop
  phone =========== session B ===========> tablet
  phone ===== session C =====[ relay ]===> hub

  Each ===== is its own QUIC/TLS 1.3 session, keyed between exactly those two
  devices. The relay forwards packets it cannot read. There is no key that
  opens A, B and C together, so the tablet never sees what went to the laptop.
```

## 4. Nothing before authentication

A device sends no manifest, no gossip and no capability traffic until the peer is
authenticated and its key is recognised: a binding of mine, a principal I use, or a
binding of a known person. An unknown key is answered only with the pairing offer of
chapter 02, if the device is accepting pairings. Bystanders learn nothing beyond a public
key. Open questions: whether even the public key should be hidden behind a per-epoch
identifier, as the discovery advertisements already are (chapter 04); and whether to allow
an opt-in open sharing mode (chapter 10 section 7) in which an unknown but authenticated
key may offer a share and nothing else for a limited time.

## 5. Secrets at rest

The protocol stores nothing off-device. Each device protects its own local store with
platform facilities (chapter 08). An optional backup (chapter 06 section 6) is encrypted
to a key only the user holds.

## 6. Settled points

- Mandatory TLS 1.3 cipher suites are `TLS_AES_128_GCM_SHA256`, `TLS_AES_256_GCM_SHA384`
  and `TLS_CHACHA20_POLY1305_SHA256`; `ed25519` is the only signature scheme
  ([ADR 0028](../adr/0028-quinn-over-rustls.md)).
- 0-RTT is not used (ADR 0028).
- The ceremony's short code is derived from the TLS exporter
  ([ADR 0029](../adr/0029-ceremony-code-from-tls-exporter.md)).
