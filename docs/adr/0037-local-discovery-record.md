# 0037. The local discovery record

**Status:** Accepted
**Date:** 2026-10-19
**Extends:** [ADR 0012](0012-decentralised-helpers.md) and
[ADR 0036](0036-stream-framing-and-message-typing.md)

## Context

Devices find each other on a LAN with mDNS/DNS-SD (ADR 0012). Two implementations can only
find each other if they agree on the service type and on what the record carries, and the
privacy goal of chapter 09 (local advertisements reveal nothing about the user) constrains
what may appear. Chapter 04 section 4.1 wants a per-epoch identifier that lets devices in one
set recognise each other without telling bystanders anything; its derivation is not
designed, and the first milestone cannot wait for it.

## Decision

1. **Service type** `_ligature._udp`. The SRV record carries the UDP port of the QUIC
   endpoint; the port is whatever the endpoint was assigned and is never fixed.
2. **Nothing identifying.** The record carries no device key, no display name of a person
   or device, no host name of the machine and nothing stable across restarts. A bystander
   learns only that some device speaking the protocol is present.
3. **One identifier for everything visible.** Each daemon draws 8 random bytes at start and
   renders them as 16 lowercase hex characters, `id`. The service instance name is
   `lp-<id>` and the SRV target host name is `lp-<id>.local`, advertised with the device's
   own addresses, so neither DNS-SD browsers nor the SRV record show anything else.
4. **TXT keys.** Exactly two are defined: `v`, the protocol version as the same number the
   ALPN token carries (`1`), and `id` as above. Receivers ignore unknown keys.
5. **`id` is a hint, never a credential.** A browser uses it to tell instances apart, drop
   stale ones and back off from one that just failed authentication. Recognition of a peer
   is the mutual raw-public-key handshake and nothing before it. For now a device dials
   every instance whose `v` it supports, with a cap on concurrent dials and a back-off per
   `id` after a failed authentication.

## Consequences

The record shape is fixed; when the per-epoch recognisable identifier of chapter 04 is
designed, it replaces the random value in `id` and nothing else changes. Until then a
stranger's daemon that we dial learns our bare device key (TLS 1.3 hides both certificates
from passive observers, not from the peer), which chapter 09 already lists as an open
question. The service type should be registered with IANA before a public release, as the
domain must be.
