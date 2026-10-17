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
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Unimplemented(what) => write!(f, "not implemented: {what}"),
        }
    }
}

impl std::error::Error for Error {}
