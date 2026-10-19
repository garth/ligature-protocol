//! Identity layer: master keys and person facts, the pairing ceremony, role
//! signatures, and the local trust store.
//!
//! Implements spec chapters 01, 02 and 06 (ADRs 0003, 0016 and 0017). A person
//! is a master signing key created on their first device. An introducer signs
//! a verified key with a role (`binding`, `principal`, `person`) in a proximity
//! ceremony; every device of that person then trusts it. There is no account
//! and no required server.
//!
//! **Status:** milestone 1 in progress.

pub mod cross_signing;
pub mod facts;
pub mod key_file;
pub mod pairing;
pub mod trust_store;
