# 0026. Rust is the reference implementation

**Status:** Accepted
**Date:** 2026-10-17

## Context

The reference implementation must run as a daemon on Linux desktops, as a service on
Android, and in part on iOS; it must speak QUIC with TLS raw public keys (ADR 0011,
ADR 0019), encode CBOR deterministically (ADR 0025), and drive Wayland, D-Bus and BLE. The
language chosen has to support every target device and all of that functionality in one
codebase, so that the reference and the shipped daemon are the same code.

ADR 0014 had held this question open as a review of several candidate languages. It was
withdrawn and removed at the project owner's request; its number is intentionally unused.

## Decision

The reference implementation is **Rust**: the existing cargo workspace under `impl/`. It is
also the production implementation. Platform bindings for Android and iOS are generated
from the Rust core when those milestones arrive. Conformance is defined by the
specification and by the CBOR test vectors committed from milestone 1 onward, which any
other implementation may run.

## Consequences

One implementation to maintain, with the best available QUIC, TLS raw-public-key, CBOR and
cryptography libraries. Contributors need Rust to work on the implementation; the
specification and schemas remain the readable account of the protocol for everyone else.
The milestone 1 plan's language decision is settled and its earlier hedge removed.
