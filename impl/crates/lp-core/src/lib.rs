//! Shared types for the Ligature Protocol reference implementation.
//!
//! This crate holds the vocabulary every other crate speaks: identifiers for
//! users and devices, the capability model (descriptors, requests, grants) and
//! the error type. It contains no I/O and no cryptography.
//!
//! **Status:** scaffold. Types are declared with documentation only; no logic
//! is implemented yet. See `docs/spec/00-overview.md` for the layering these
//! modules correspond to.

pub mod capability;
pub mod device;
pub mod envelope;
pub mod error;
pub mod identity;
pub mod paths;

pub use error::Error;

/// Convenience alias used throughout the workspace.
pub type Result<T> = core::result::Result<T, Error>;
