//! The device's local store of signed facts. Spec chapter 06.
//!
//! There is no shared database: this store *is* the truth as far as this
//! device knows. It holds the owner's master public key, known persons, device
//! records with their role signatures, revocations and abandonments, issued
//! grants, group memberships and relays. After authentication at the start of
//! every session the device gossips what the peer lacks, revocations first,
//! and applies what the peer sends if it verifies.

/// The persistent local store of signed facts.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct TrustStore;

/// One signed fact as it travels in gossip: a role signature, a revocation or
/// abandonment, a group membership change, a standing grant or a relay list.
///
/// Not yet implemented.
#[derive(Debug)]
pub struct SignedFact;
