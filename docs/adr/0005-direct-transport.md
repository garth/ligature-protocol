# 0005. Direct device-to-device transport with local discovery and a WireGuard overlay

**Status:** Proposed
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

Works on a LAN with no internet. Remote use requires the user (or a provider) to run a
Headscale-compatible coordinator, which is a setup burden. Discovery advertisements must
be designed to avoid leaking identity to bystanders. BLE provides a bridge to iOS
accessory profiles (ADR 0010).
