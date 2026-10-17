# Prior Art

Projects and protocols that LP learns from, bridges to, or overlaps with. Each entry notes
what LP takes from it and where LP differs. This list is a starting point and should grow.

## Device ecosystems

**KDE Connect**: Mature Linux-to-Android device integration (notifications, clipboard,
file transfer, remote input, media control) over a TLS-secured LAN protocol with mDNS
discovery. LP takes its capability list and plugin architecture as a reference. LP differs
by having a user identity above the device, group encryption instead of pairwise TLS, and
a remote path. LP intends to bridge to KDE Connect.

**Valent**: A GNOME-oriented implementation of the KDE Connect protocol. Useful evidence
that the KDE Connect protocol is implementable independently.

**Tether**: A Rust reimplementation of the KDE Connect protocol with a focus on
modularity. Close to LP's implementation language and a candidate source of protocol
details for the KDE Connect bridge.

**LocalSend**: Cross-platform local file sharing over HTTPS with mDNS discovery and no
accounts. LP's file capability should interoperate with it where possible.

**Apple Continuity**: The proprietary benchmark: Handoff, Universal Clipboard, AirDrop,
Sidecar, phone calls on Mac. LP aims at equivalent user experience on open platforms.

## Identity and encryption

**Matrix**: Source of the cross-signing model for device trust (ADR 0003) and a widely
deployed example of per-device keys with user-level verification.

**Germ (and the AC protocol)**: An end-to-end encrypted messenger built on atproto identity
plus MLS with a separate transport. LP's identity + MLS + transport layering follows this
pattern (ADR 0004).

**MLS (RFC 9420) and OpenMLS**: The group encryption standard and the Rust implementation
LP plans to use.

**Signal**: Reference point for pairwise double-ratchet sessions, which LP rejected in
favour of MLS groups.

## Shared state and schemas

**AT Protocol Spaces**: Permissioned data in a PDS, currently alpha. LP's shared state store
(ADR 0006).

**AT Protocol Lexicons**: Schema system used for all LP records (ADR 0007).

**Syncthing**: Peer-to-peer file synchronisation with device IDs derived from TLS
certificates and introducer-based device discovery. A possible bridge target and a design
reference for device-to-device sync without a server.

## Recovery

**Dark Crystal**: Social recovery using Shamir shares distributed to trusted peers over
Secure Scuttlebutt. Direct inspiration for ADR 0009; LP adds in-person verification, a
time delay with veto, liveness checks and share refresh.

**Argon / Ethereum social recovery wallets**: Guardian-based account recovery with time
delays; evidence that delay-plus-veto is a workable pattern.

## Transport

**WireGuard, Tailscale and Headscale**: The overlay network and self-hostable coordination
server LP targets for remote connectivity (ADR 0005).

**mDNS/DNS-SD (RFC 6762, RFC 6763)**: Local service discovery.

## Smart home and iOS interop

**Matter / Thread**: Open smart-home standard LP intends to bridge to for hub and sensor
capabilities.

**Apple BLE accessory profiles (ANCS, AMS) and Bluetooth HFP/MAP/PBAP**: The only way for a
non-Apple device to receive iPhone notifications, control media, and access calls, messages
and contacts without an app. Basis for LP's iPhone support (ADR 0010).

**EU Digital Markets Act interoperability**: Emerging iOS APIs mandated in the EU for
third-party device interoperability; LP will adopt them where available.

## Calendar and contacts

**CalDAV / CardDAV**: Existing standards for calendar and contact sync. LP will not
reinvent these and expects to point devices at a CalDAV/CardDAV server instead.
