//! The workspace-wide error type.

/// Errors that can occur anywhere in the reference implementation.
///
/// Variants will be added as protocol logic lands. Keeping a single error enum
/// in `lp-core` lets the daemon and CLI report failures uniformly.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A feature that is specified but not yet implemented.
    Unimplemented(&'static str),
    /// A key string that is not a base58btc multibase Ed25519 public key.
    InvalidKey(String),
    /// A signature that is malformed or does not verify.
    InvalidSignature(String),
    /// The operating system's random source failed.
    Randomness(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Unimplemented(what) => write!(f, "not implemented: {what}"),
            Error::InvalidKey(what) => write!(f, "invalid key: {what}"),
            Error::InvalidSignature(what) => write!(f, "invalid signature: {what}"),
            Error::Randomness(what) => write!(f, "randomness unavailable: {what}"),
        }
    }
}

impl std::error::Error for Error {}
