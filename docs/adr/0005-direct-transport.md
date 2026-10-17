# 0005. Direct device-to-device transport with local discovery and a WireGuard overlay

**Status:** Superseded by [ADR 0011](0011-quic-application-transport.md),
[ADR 0012](0012-decentralised-untrusted-helpers.md) and
[ADR 0013](0013-wireguard-optional-integration.md)
**Date:** 2026-10-17

## Context

Local-first operation requires that devices on the same network find and talk to each
other with no server. Away from home, devices still need a path to each other. Routing
real-time traffic through the PDS would make the PDS a bottleneck, a privacy liability and
a hard dependency.

## Decision

Devices discover each other locally with mDNS/DNS-SD and BLE and connect directly. When
apart they connect over a WireGuard overlay compatible with Headscale, so users can
self-host coordination. Real-time traffic never routes through a PDS.

## Consequences

*Superseded on 2026-10-17.* The local-discovery and "never through a PDS" parts of this
decision survive in ADR 0011 and ADR 0012. The WireGuard overlay was dropped because it
needs a virtual network interface, which takes the single VPN slot on Android and iOS and
needs elevated privileges on desktops; ADR 0013 keeps it as an optional integration.

Original consequences follow.

Works on a LAN with no internet. Remote use requires the user (or a provider) to run a
Headscale-compatible coordinator, which is a setup burden. Discovery advertisements must
be designed to avoid leaking identity to bystanders. BLE provides a bridge to iOS
accessory profiles (ADR 0010).
