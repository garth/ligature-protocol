//! Identity layer: master keys, the pairing ceremony, role signatures, the
//! local trust store, and optional key publication.
//!
//! Implements spec chapters 01, 02 and 06 (ADRs 0003, 0016 and 0017). A person
//! is a master signing key created on their first device. An introducer signs
//! a verified key with a role (`binding`, `principal`, `person`) in a proximity
//! ceremony; every device of that person then trusts it. There is no account
//! and no required server.
//!
//! **Status:** scaffold. Stub modules and documentation only.

pub mod cross_signing;
pub mod pairing;
pub mod publication;
pub mod trust_store;
