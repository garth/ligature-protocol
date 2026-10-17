//! Cryptographic primitives: signing keys and raw-public-key TLS material.
//!
//! Implements the key handling behind spec chapters 01 to 03 (ADRs 0017 and
//! 0019). The pairwise QUIC/TLS 1.3 session is the end-to-end encryption
//! layer; this crate produces the device key material that `lp-transport`
//! presents as a TLS raw public key (RFC 7250) and verifies, and the signing
//! operations `lp-identity` uses for role signatures. There is no group key.
//!
//! **Status:** scaffold. No cryptographic dependency is pulled in yet.

pub mod session;
pub mod signing;
