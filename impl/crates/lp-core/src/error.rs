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
    /// Bytes or text that do not decode as the expected form.
    Decode(String),
    /// A fact whose signer is not what the fact's type and role require, or
    /// whose chain does not reach the expected master key.
    Chain(String),
    /// A file or directory could not be read or written.
    Io(String),
    /// The local store failed.
    Store(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Unimplemented(what) => write!(f, "not implemented: {what}"),
            Error::InvalidKey(what) => write!(f, "invalid key: {what}"),
            Error::InvalidSignature(what) => write!(f, "invalid signature: {what}"),
            Error::Randomness(what) => write!(f, "randomness unavailable: {what}"),
            Error::Decode(what) => write!(f, "cannot decode: {what}"),
            Error::Chain(what) => write!(f, "trust chain: {what}"),
            Error::Io(what) => write!(f, "i/o: {what}"),
            Error::Store(what) => write!(f, "store: {what}"),
        }
    }
}

impl std::error::Error for Error {}
