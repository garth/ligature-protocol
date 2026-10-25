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

**Synergy, Barrier and Input Leap**: One keyboard and mouse across computers, the pointer
crossing from one screen to the next. Synergy began as a reimplementation of Chris
Schoeneman's CosmoSynergy; Symless's Synergy 1.9 was forked as Barrier, and Barrier as
Input Leap, which keeps Synergy's key message. LP takes that key model for its input
family: each key carries its meaning on the controller (a key ID, a Unicode character or a
named key), the modifier mask and the physical key, and the driven device finds its own key
for the meaning ([ADR 0043](adr/0043-keys-carry-their-meaning.md)). LP differs in carrying
input over its authenticated per-device sessions rather than a server and clients, and in
the edge layout being each device's own setting.

## Identity and encryption

**Matrix**: Source of the cross-signing model for device trust (ADR 0003, ADR 0017): a
master key signs self-signing and user-signing keys, which sign devices and other users.
LP keeps the structure and drops the server-side user ID above it.

**Bluetooth and Wi-Fi Easy Connect pairing**: Proximity ceremonies with a displayed code
or QR, verified by the user comparing both sides. LP's ceremony (chapter 02) is the same
gesture with a signature attached.

**TLS 1.3 raw public keys (RFC 7250)**: Certificate-free mutual authentication; with
ephemeral key exchange it is LP's end-to-end encryption layer (ADR 0019).

## State and schemas

**Matrix device lists and Secure Scuttlebutt feeds**: Both distribute per-device signed
facts (device keys, revocations, posts) by replication rather than through a shared
database. LP's gossip of signed facts (spec chapter 06) follows the same shape.

**D-Bus and Java naming**: Reverse-DNS type names such as `org.ligatureprotocol.device`
follow this long-standing convention (ADR 0020).

**JSON Schema**: LP's schema language (ADR 0020).

**CBOR (RFC 8949) and COSE (RFC 9052)**: The IETF's binary encoding and its signing and
encryption layer, used by WebAuthn, CWT and much constrained-device work. LP's wire
encoding (ADR 0025); COSE is a reference for signed-object layout if LP's own signed-fact
format ever needs to interoperate.

**Syncthing**: Peer-to-peer file synchronisation with device IDs derived from TLS
certificates and introducer-based device discovery. A possible bridge target and a design
reference for device-to-device sync without a server.

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

**WireGuard**: The kernel-level VPN protocol. LP's optional integration generates
WireGuard/Headscale configurations from the device list for users who want a full
home-network overlay (ADR 0013).

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
