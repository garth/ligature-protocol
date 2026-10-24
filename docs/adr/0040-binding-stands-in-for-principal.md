# 0040. On a single-user device the binding offers the device-scoped capabilities

**Status:** Accepted
**Date:** 2026-10-24
**Extends:** [ADR 0023](0023-group-devices-sessions-and-authority.md) and
[ADR 0015](0015-shared-devices-and-groups.md)

## Context

Chapter 10 gives every capability a scope: a `user`-scoped capability belongs to a person's
binding on a device, a `device`-scoped one to the device principal, which is the device
itself with its own key and a system-level daemon. The reference implementation runs one
daemon per account, `ligatured` as a user service, holding that account's binding key and
offering the user-scoped families (clipboard, notifications, files). There is no principal
service: on Linux it would be a system unit, on macOS a launchd daemon, on Android a
privileged component, and none exists yet.

Milestone 7 brings the first device-scoped family, `input` (chapter 05 section 3g): a
keyboard and mouse belong to the device, and the person present on it answers a request to
drive it. Milestone 8's `screen` is the second. Both need an owner in the manifest today.
Waiting for a principal service would hold two milestones for a piece of plumbing whose
shape depends on each platform's privilege model; inventing a second key in the account
daemon to play the principal would put a device key where chapter 10 says the device has
one, and a later principal service would have to inherit or revoke it.

## Decision

1. **The account's daemon offers the device-scoped capabilities on a device with one
   account.** Its manifest lists `input`, `screen` and any later device-scoped family
   beside the user-scoped ones, each descriptor marked by its `scope` alone; nothing else
   in the manifest or the stream header says who stands behind it.
2. **A peer treats such a descriptor exactly as one from a principal.** The question a
   device-scoped request raises is who is present on the device (chapter 10 section 6),
   and on a person's own single-user desktop that is the person. Between one person's
   own devices the use is therefore implicit, as for user-scoped capabilities under
   [ADR 0015](0015-shared-devices-and-groups.md); across people it needs the present
   person's approval or the device's policy, whichever daemon answers.
3. **The principal service, when it arrives, offers the same descriptors and the binding
   stops.** The owner of the capability changes from the binding's key to the device's,
   the manifest's sender changes with it, and nothing on the stream changes. A device
   with several accounts never has a binding stand in: it waits for the principal
   service, since two bindings cannot both own one keyboard.

## Consequences

Milestones 7 and 8 proceed on today's daemon, and chapter 10's ownership model is
unchanged: the binding is standing in, not redefined. A peer's logic is written once,
against the descriptor's scope, and is right both before and after the principal service.
What is lost until then is the shared-device case, where a device-scoped capability must
be offered whoever is signed in, which is exactly the case the principal service exists
for. A reader of a manifest cannot tell a binding standing in from a principal, which is
the point: there is nothing to migrate when the split comes.
