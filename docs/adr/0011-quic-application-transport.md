# 0011. Application-level QUIC transport authenticated by device keys

**Status:** Proposed; the MLS layer above QUIC is superseded by [ADR
0019](0019-pairwise-least-exposure-encryption.md) (the QUIC/TLS session is the end-to-end layer) and
the library choice is in [ADR 0028](0028-quinn-over-rustls.md); the transport decision stands
**Date:** 2026-10-17
**Supersedes:** the transport part of [ADR 0005](superseded/0005-direct-transport.md)

## Context

ADR 0005 relied on a WireGuard overlay for remote connectivity. WireGuard needs a virtual
network interface: on Android and iOS that occupies the single VPN slot and conflicts with
users' existing VPNs, and on desktops it needs elevated privileges. LP needs a transport
that runs entirely in user space inside an ordinary application, multiplexes several
capabilities without head-of-line blocking, survives network changes, and authenticates
peers with the device keys LP already has. Options considered: TCP with TLS and a custom
multiplexer, WebRTC data channels, libp2p's transport stack, and QUIC (RFC 9000). iroh
demonstrates QUIC dialled by public key with hole punching and relays.

## Decision

LP's transport is application-level QUIC. One QUIC connection per device pair; one stream
per capability so a bulk transfer never stalls interactive traffic. Peers authenticate
with their LP device keys as TLS raw public keys (RFC 7250); there are no certificate
authorities. Connection migration is used so sessions survive Wi-Fi to mobile-data
changes. MLS (ADR 0004) remains the end-to-end layer above QUIC; QUIC/TLS is defence in
depth for the hop.

## Consequences

No kernel interface, no privileges, no VPN-slot conflict. Multiplexing and migration come
from QUIC rather than from LP-specific code. The reference implementation needs a QUIC
library that supports raw public keys, which narrows the choice. Every connection carries
two encryption layers, which costs CPU but keeps direct and relayed paths uniform.
Relays (ADR 0012) forward QUIC packets or MLS ciphertext and need no TLS termination.
