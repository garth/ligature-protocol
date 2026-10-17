# 0024. Device security is out of scope

**Status:** Proposed
**Date:** 2026-10-17
**Supersedes:** items 4 and 5 of [ADR 0023](0023-group-devices-sessions-and-authority.md)

## Context

ADR 0023 introduced sensitivity tiers for capability families, per-use confirmation on
another device for the elevated tier when acting from a person session on a group device,
mandatory auto-lock, and a rule that a binding on a group device is never an introducer.
Each of these is a device-security policy: it exists to protect a person from leaving a
shared screen unlocked. The same reasoning would have the protocol mandate lock screens on
phones, which it has no business doing. A person who runs their phone or laptop without a
password has made a choice about their own device network, and the protocol should respect
it rather than second-guess it. The never-introducer rule also contradicted ADR 0017, which
leaves introducer status to the user's configuration.

## Decision

1. **The protocol does not cover device security.** How a device authenticates its local
   user, whether it has a lock screen, when it idles to lock, and how it protects its local
   store are the device's and the user's concern. Conformance never depends on them.
2. **The protocol defines authority and trust only.** Who is authenticated behind a request
   (ADR 0023 items 2 and 3, which stand) and which keys are recognised (ADR 0017). A
   *present* session is whatever the device's operating system reports; the protocol does
   not ask how it was unlocked.
3. **Sensitivity tiers, elevated-tier confirmation and auto-lock mandates are removed.** A
   person session on a group device acts with that person's full authority, exactly like
   any other of their devices, for as long as the device considers it present.
4. **Introducer status stays the user's choice.** Implementations should default a binding
   on a group device to not being an introducer, and may say why, but the user can change
   it.
5. **Platform guidance remains guidance.** Chapter 08 may recommend keystores and secret
   services for the local store; none of it is normative.

## Consequences

The protocol is smaller and does not moralise. The authority rule from ADR 0023 still
guarantees that a group device with nobody signed in reaches nothing of anyone's; what a
signed-in person exposes by walking away is their own device-security choice, as it is on
their phone. The `sensitivity` field leaves the capability schema, chapter 05 loses its
tier section, and the threat model states the boundary explicitly: an unlocked device in
an attacker's hands is out of scope by the user's decision, not by oversight.
