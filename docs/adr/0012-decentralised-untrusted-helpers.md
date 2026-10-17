# 0012. Decentralised, replaceable, untrusted connectivity helpers

**Status:** Proposed
**Date:** 2026-10-17
**Supersedes:** the remote-connectivity part of [ADR 0005](0005-direct-transport.md)

## Context

Locally, mDNS/DNS-SD and BLE plus direct QUIC need no server. Remotely, three problems
remain: finding where a device is now, coordinating a hole punch through NAT, and
relaying when hole punching fails (common on mobile and CGNAT networks). Any component
that solves these could become a central dependency or a trusted party, which LP's
principles forbid. iroh (pkarr records in the BitTorrent Mainline DHT, DERP-style relays)
and Tailscale (DERP) show these roles can be filled by replaceable services.

## Decision

Remote connectivity uses three helper roles: discovery, hole-punch coordination and
relaying. Discovery is served by signed address records keyed by the device public key
and published to a DHT (Mainline via pkarr), and/or by addresses and relays listed in the
user's DID document or Space records. Relays may be run by anyone; the default personal
relay is the user's always-on home hub; clients can list several; a project-run relay may
exist as a swappable convenience default. The governing rule, stated verbatim in the spec:

> Any helper may be operated by anyone, is listed in the user's own records, and is never
> trusted with plaintext or identity.

## Consequences

A malicious helper can at worst deny service; clients route around it. Discovery-record
spoofing and DHT poisoning are defeated by signatures and by raw-public-key authentication
on the connection itself. Helpers learn addresses and timing, so traffic analysis by a
helper is a known, undefended exposure in this version. Users without a home hub depend on
someone's relay, which is why a convenience default exists and why it must be trivially
replaceable. The DHT adds a dependency on a public network that LP does not control.
