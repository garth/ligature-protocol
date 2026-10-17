# 0033. Supported platforms

**Status:** Accepted
**Date:** 2026-10-17

## Context

The protocol is for the devices a person or a household actually owns. The platform list
determines what the reference implementation must run on and which capabilities each
platform can offer, since operating systems differ in what they let a third-party daemon
do. macOS was added to the list on 2026-10-17; everything in the design already fits it
(a user-space daemon, QUIC, Bonjour and CoreBluetooth are all available, and with no VPN
component there is no network-extension entitlement to obtain).

## Decision

The target platforms, in order of implementation:

1. **Linux** desktops and laptops (Wayland, Hyprland first): the reference platform.
2. **macOS** desktops and laptops: a full peer, with per-account binding daemons and a
   system principal daemon under launchd.
3. **Android** phones and tablets.
4. **Hubs**: headless Linux devices, also the default personal relay.
5. **iOS** within what the platform allows, through BLE accessory profiles and regional
   interoperability APIs, as ADR 0010 describes.

Each platform's constraints on daemons, presence, clipboard, notifications, input, display
and keystores are recorded in chapter 08 and nowhere else.

## Consequences

The Rust implementation (ADR 0026) must build and run on Linux and macOS from milestone 2
on, with Android following. Capability families are specified once; a platform that cannot
offer one simply does not advertise it. Chapter 08 gains a macOS section; the device
schema's platform examples and `lp-core::Platform` gain `macos`.
