# 0031. The split key structure is the design and is implemented from the start

**Status:** Accepted
**Date:** 2026-10-17
**Amends:** item 3 of [ADR 0017](0017-master-key-identity-and-proximity-ceremony.md)

## Context

ADR 0017 described a default key configuration with a master key, a self-signing key held
by introducers and a user-signing key for known persons, and also allowed a "simplest"
configuration in which one key played all three parts. The milestone 1 plan proposed
building the single-key form first. A structure the full design requires is not proven by
a proof of concept that avoids it, and two configurations mean two code paths and two sets
of vectors.

## Decision

1. Every person has three keys: a **master key**, a **self-signing key** and a
   **user-signing key**, the latter two signed by the master key. This is the only
   structure; there is no single-key form.
2. The proof of concept implements all three from the start.
3. What remains configurable, to the user's requirements: where the master private key
   lives (on the first device, or offline or derived from a passphrase) and which bindings
   are introducers and therefore hold the self-signing key.

## Consequences

The ceremony, gossip and trust-store code exercise the real chains from day one, and the
first conformance vectors cover them. Chapter 01's open question about the default
configuration is closed. Milestone 1 task A2 becomes the split structure rather than a
shortcut.
