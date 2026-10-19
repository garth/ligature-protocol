//! Shared types for the Ligature Protocol reference implementation.
//!
//! This crate holds the vocabulary every other crate speaks: identifiers for
//! users and devices, the capability model (descriptors, requests, grants) and
//! the error type. It contains no I/O and no cryptography.
//!
//! **Status:** milestone 1 in progress. See `docs/spec/00-overview.md` for the layering these
//! modules correspond to.

pub mod capability;
pub mod device;
pub mod envelope;
pub mod error;
pub mod identity;
pub mod key;
pub mod paths;

pub use error::Error;
pub use key::{DeviceKey, GroupKey, PersonKey, PublicKey};

/// Convenience alias used throughout the workspace.
pub type Result<T> = core::result::Result<T, Error>;
