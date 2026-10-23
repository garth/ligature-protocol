//! Discovery and transport layer.
//!
//! Implements spec chapter 04 (ADRs 0011 and 0012). Devices connect directly
//! over application-level QUIC (RFC 9000), authenticating with their LP device
//! keys as TLS raw public keys (RFC 7250). Locally they find each other with
//! mDNS/DNS-SD and BLE and need no server. Remotely they use replaceable,
//! untrusted helpers in three roles: discovery, hole-punch coordination and
//! relaying.
//!
//! Design rule: any helper may be operated by anyone, is listed in the user's
//! own records, and is never trusted with plaintext or identity. The QUIC/TLS
//! session is itself the end-to-end encryption layer (ADR 0019); relays
//! forward packets and terminate nothing.
//!
//! WireGuard is not part of this crate or of the protocol; see the non-core
//! `wireguard-integration` crate.
//!
//! QUIC with raw public keys and mDNS discovery are implemented; BLE, the
//! remote discovery and the relay client are placeholders.

pub mod ble;
pub mod discovery;
pub mod manager;
pub mod mdns;
pub mod quic;
pub mod relay;
pub mod stream;
