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

**AT Protocol Spaces**: Permissioned data in a PDS, currently alpha. Considered as LP's
shared state store (ADR 0006) and dropped when it became clear no shared state was needed
(ADR 0016).

**Matrix device lists and Secure Scuttlebutt feeds**: Both distribute per-device signed
facts (device keys, revocations, posts) by replication rather than through a shared
database. LP's gossip of signed facts (spec chapter 06) follows the same shape.

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

## Transport and connectivity

**iroh**: A Rust networking library that dials peers by public key over QUIC, with hole
punching, relays, and discovery through signed records in the BitTorrent Mainline DHT (via
pkarr) or DNS. The closest existing design to LP's transport (ADR 0011, ADR 0012); its
endpoint abstraction is a candidate for direct reuse in the production core.

**libp2p**: The modular peer-to-peer networking stack from IPFS: transports (QUIC among
them), peer identity from public keys, NAT traversal (DCUtR), relays (circuit relay v2) and
a Kademlia DHT. A reference for how to separate transport, identity and discovery; heavier
than LP needs.

**pkarr and the BitTorrent Mainline DHT**: pkarr publishes signed DNS-style records keyed
by an Ed25519 public key into Mainline, the largest deployed DHT. LP's DHT-based discovery
option (spec chapter 04 section 5.1) follows this pattern.

**Tailscale and Headscale (DERP relays)**: Tailscale's DERP relays forward WireGuard
packets between peers that cannot hole-punch, are run by anyone, and are trusted with
nothing but addresses. LP's relay role is modelled on DERP's trust posture (ADR 0012).
Headscale is the self-hostable coordination server; LP's optional WireGuard integration
(ADR 0013) generates configurations compatible with it.

**QUIC (RFC 9000) and TLS raw public keys (RFC 7250)**: The transport protocol and the
certificate-free authentication mode LP uses to bind connections to device keys.

**mDNS/DNS-SD (RFC 6762, RFC 6763)**: Local service discovery.

**WireGuard**: The kernel-level VPN protocol. Formerly LP's remote transport (ADR 0005,
superseded); now an optional integration for users who want a full home-network overlay.

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
