# 0014. Reference implementation language

**Status:** Proposed (under review; no restructuring until confirmed)
**Date:** 2026-10-17

## Context

The repository currently holds a Rust cargo workspace scaffold (`impl/`). The purpose of a
*reference* implementation is to make the specification unambiguous and testable, which
favours readability and reach over performance. A *production* implementation must run on
Linux, Android and, in part, iOS, and favours a single core shared across platforms. These
two goals may be served by different languages.

Alternatives considered:

| Language | For | Against |
| --- | --- | --- |
| **TypeScript** | Widest contributor base; very readable; runs in Node, Deno, Bun and browsers; atproto's own reference libraries are TypeScript; easy to pair with JSON test vectors | Not suitable for a battery-sensitive daemon on Android or for iOS; QUIC and MLS library maturity is weaker than Rust's |
| **Go** | Simple, readable, good networking stack (quic-go), single static binaries, easy daemons | Mobile integration is awkward (gomobile); MLS libraries less mature; GC and binary size on mobile |
| **Rust** | Best QUIC (quinn, iroh) and MLS (OpenMLS) libraries; no GC; UniFFI gives Kotlin and Swift bindings from one core; the current scaffold | Steeper learning curve for spec readers; slower iteration while the spec is in flux |
| **Kotlin Multiplatform** | First-class on Android; shares code with iOS and JVM desktops | Linux daemon story is weak; smaller networking and crypto ecosystem for QUIC/MLS |
| **C# / .NET** | Mature, cross-platform runtime; MAUI for mobile | Little traction with the Linux desktop audience LP targets; MLS and raw-public-key QUIC support thin |
| **Zig** | Small binaries, C interop, no hidden allocations | Pre-1.0; almost no QUIC or MLS ecosystem; tiny contributor pool |

## Decision (proposed)

Write the **reference implementation in TypeScript**, optimised for readability, alongside
**language-neutral conformance test vectors** (JSON fixtures for every protocol operation)
that any implementation can run. Build the **production core in Rust** later, exposing
Kotlin and Swift bindings through UniFFI, validated against the same test vectors.

The existing Rust scaffold is kept as-is until this ADR is confirmed. If accepted, it is
repositioned as the production core and a `ref/` or `reference/` TypeScript tree is added;
the Lexicons and test vectors become the contract between the two.

## Consequences

Two implementations means the spec is exercised twice, which catches ambiguity early, at
the cost of maintaining both. The test-vector suite becomes a first-class deliverable and
the real definition of conformance. Contributors can read the protocol in TypeScript
without learning Rust. The decision on milestone 1 (two Linux devices sharing a clipboard)
depends on this: a TypeScript reference can demonstrate it on Linux, but the daemon users
install will be the Rust one.
