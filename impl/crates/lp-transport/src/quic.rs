//! QUIC connections between devices. Spec chapter 04 section 3.
//!
//! Planned responsibilities:
//!
//! - One QUIC connection per device pair, dialled directly or through a relay.
//! - One stream per capability so bulk transfers never stall interactive ones.
//! - Authenticate the peer's TLS raw public key (RFC 7250) against the device
//!   set or an active grant. No certificate authorities.
//! - Use connection migration so sessions survive network changes.

/// The local QUIC endpoint: listens, dials and owns live connections.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct Endpoint;

/// A QUIC connection to one peer device.
///
/// Not yet implemented.
#[derive(Debug)]
pub struct Connection;

/// A stream within a [`Connection`], dedicated to one capability.
///
/// Not yet implemented.
#[derive(Debug)]
pub struct CapabilityStream;
