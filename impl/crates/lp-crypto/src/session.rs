//! Raw-public-key TLS material for QUIC sessions. Spec chapter 03.
//!
//! Planned responsibilities:
//!
//! - Present the device key as a TLS raw public key (RFC 7250).
//! - Verify a peer's raw public key and hand it to `lp-identity` for the
//!   role check (binding of mine, principal I use, binding of a known person).
//! - Enforce mandatory cipher suites and key-update policy.

/// TLS configuration for one device's QUIC endpoint.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct SessionConfig;
