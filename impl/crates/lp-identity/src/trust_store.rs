//! The device's local store of signed facts. Spec chapter 06.
//!
//! There is no shared database: this store *is* the truth as far as this
//! device knows. It holds the identity's public cross-signing keys, trusted
//! device records with their cross-signatures, revocations, issued grants and
//! group memberships. On every new session the device gossips what the peer
//! lacks, revocations first, and applies what the peer sends if it verifies.

/// The persistent local store of signed facts.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct TrustStore;

/// One signed fact as it travels in gossip: a device binding, a revocation, a
/// group membership change or a standing grant.
///
/// Not yet implemented.
#[derive(Debug)]
pub struct SignedFact;
