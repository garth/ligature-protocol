//! Discovery and transport layer.
//!
//! Implements spec chapter 04 (ADR 0005): mDNS/DNS-SD and BLE for local
//! discovery, direct connections between devices, and a WireGuard overlay for
//! remote connectivity. Authentication is derived from the MLS group, not from
//! the transport.
//!
//! **Status:** scaffold. Stub modules and documentation only.

pub mod ble;
pub mod connection;
pub mod mdns;
