# 0028. quinn over rustls as the QUIC stack

**Status:** Accepted
**Date:** 2026-10-17

## Context

The transport is QUIC authenticated by TLS raw public keys in both directions (ADR 0011,
ADR 0019) with Ed25519 device keys (ADR 0027), and the pairing ceremony needs the TLS
exporter (ADR 0029). The implementation is Rust (ADR 0026). Few TLS libraries implement
RFC 7250 for both client and server; the choice of stack is therefore a real constraint,
not a convenience.

## Decision

1. The QUIC stack is **quinn** with **rustls** as its TLS provider. rustls negotiates the
   `server_certificate_type` and `client_certificate_type` extensions, accepts a
   SubjectPublicKeyInfo in place of a certificate chain, and lets custom verifiers opt
   into raw public keys on both sides; quinn exposes the connection's exporter.
2. **Mutual authentication is mandatory.** Both endpoints present an Ed25519 raw public key
   and reject any other certificate type. `ed25519` is the only TLS signature scheme
   offered.
3. **Mandatory cipher suites** are rustls's TLS 1.3 defaults: `TLS_AES_128_GCM_SHA256`,
   `TLS_AES_256_GCM_SHA384`, `TLS_CHACHA20_POLY1305_SHA256`.
4. **0-RTT is not used.** Capability invocations are not idempotent and a LAN round trip
   is cheap.
5. **Task B1 of milestone 1 is the verification gate.** Before anything is built on the
   stack, a minimal client and server must demonstrate: mutual raw-public-key
   authentication; the verifier receiving the peer's raw key bytes in a form that renders
   to the multibase string for trust-store lookup; `export_keying_material` with a custom
   label and context after the handshake; and connection migration with raw public keys.

## Consequences

The implementation takes a dependency on rustls's raw-public-key API, which is newer than
its certificate path; B1 exists to catch gaps early. If B1 fails on a point that cannot be
worked around, the fallbacks are quiche (BoringSSL, which implements RFC 7250) or s2n-quic,
each requiring the exporter check again, and this ADR is superseded. The chapter 03 open
questions on cipher suites and 0-RTT are closed.
