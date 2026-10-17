//! Identity layer: DID resolution and device key cross-signing.
//!
//! Implements spec chapters 01, 02 and 06 (ADRs 0002, 0003 and 0016). This
//! crate will resolve `did:plc` and `did:web` documents, cache them, manage
//! the cross-signing chain from the user's master key down to per-device keys,
//! and keep the device's local trust store of signed facts.
//!
//! **Status:** scaffold. Stub modules and documentation only.

pub mod cross_signing;
pub mod resolver;
pub mod trust_store;
