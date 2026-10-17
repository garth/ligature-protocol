//! Identity layer: DID resolution and device key cross-signing.
//!
//! Implements spec chapters 01 and 02 (ADRs 0002 and 0003). This crate will
//! resolve `did:plc` and `did:web` documents, cache them, and manage the
//! cross-signing chain from the user's master key down to per-device keys.
//!
//! **Status:** scaffold. Stub modules and documentation only.

pub mod cross_signing;
pub mod resolver;
