# 0013. WireGuard / Headscale as an optional integration, not a transport

**Status:** Proposed
**Date:** 2026-10-17
**Supersedes:** [ADR 0005](superseded/0005-direct-transport.md)

## Context

ADR 0005 made a Headscale-compatible WireGuard overlay the remote transport. WireGuard
needs a virtual network interface, which takes the single VPN slot on Android and iOS,
conflicts with users' existing VPNs, and needs elevated privileges on desktops. ADR 0011
and ADR 0012 replace it. Many LP users nonetheless want full network access to their home
(SSH, NAS, media servers), which is exactly what a WireGuard mesh provides and what LP's
capability model does not.

## Decision

WireGuard is not a core dependency and LP never depends on it. LP can, as an optional
integration, generate Headscale/WireGuard peer configurations from the user's device list
so that users who want a home network overlay can set one up with the keys and devices LP
already knows about. The integration lives outside the core crates and is clearly marked
non-core.

## Consequences

The protocol works identically with or without WireGuard. Users who want SSH-to-home get a
one-step path to a mesh they already trust. The integration must track Headscale's config
format and WireGuard key handling, which is maintenance outside LP's core concern. The
previous ADR is marked Superseded.
