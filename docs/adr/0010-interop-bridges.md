# 0010. Interoperate with and bridge to existing protocols rather than replace them

**Status:** Proposed
**Date:** 2026-10-17

## Context

Users already run KDE Connect, LocalSend, Syncthing, CalDAV/CardDAV servers and Matter
devices. iPhones cannot run an LP daemon but do speak standard BLE accessory profiles
(ANCS, AMS, HFP, MAP, PBAP) and, in the EU, are gaining Digital Markets Act
interoperability APIs.

## Decision

LP bridges to or coexists with existing protocols where practical instead of replacing
them. A bridge is a device that advertises an LP capability on behalf of an external
system. iPhone support is delivered through BLE accessory profiles and DMA APIs rather
than a native LP implementation.

## Consequences

LP is useful before it has its own implementation of every feature. Bridges inherit the
limitations and security properties of the bridged protocol, which must be stated per
bridge. iPhone functionality will be narrower than Android and will vary by region.
