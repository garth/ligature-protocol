# 04. Discovery and Transport

**Status:** Draft.

## 1. Intent

Devices connect to each other directly. Locally they find each other with mDNS/DNS-SD and
BLE; when apart they use a WireGuard overlay compatible with Headscale. Real-time traffic
never routes through a PDS. See [ADR 0005](../adr/0005-direct-transport.md).

## 2. Local discovery: mDNS/DNS-SD

Each device advertises a DNS-SD service instance. The TXT record carries enough to
identify the device set without revealing the user's identity to bystanders. Open question:
the exact privacy design of the advertisement (likely: a per-epoch hashed identifier).

## 3. Local discovery: BLE

BLE advertisements allow phones and low-power devices to be discovered and woken. BLE is
also the path to iPhone accessory profiles (chapter 08). Open question: whether LP defines
its own GATT service or only uses BLE to bootstrap an IP connection.

## 4. Remote connectivity: WireGuard overlay

When devices are not on the same network they connect over a WireGuard overlay. LP aims to
be compatible with Headscale so users can self-host coordination. Open question: how much
of the Tailscale/Headscale coordination protocol LP depends on versus merely coexists with.

## 5. Connection establishment

After discovery a device opens an authenticated connection to a peer. Authentication is
derived from the MLS group (chapter 03), not from the transport. Open question: the
session protocol on top of TCP/QUIC and whether QUIC is mandatory.

## 6. Wake and presence

Devices should be able to wake peers and learn their presence cheaply. Open question: how
presence interacts with battery constraints on mobile platforms.

## 7. Open questions

- NAT traversal when no overlay is configured: hole punching, relays, or none?
- How are multiple network paths to the same peer ranked?
