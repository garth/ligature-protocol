//! DID and handle resolution. Spec chapter 01.
//!
//! Planned responsibilities:
//!
//! - Resolve `did:plc` via the PLC directory and `did:web` via HTTPS.
//! - Resolve handles via DNS TXT and `/.well-known/atproto-did`.
//! - Cache documents for offline operation and surface staleness.

/// Resolves DIDs and handles to DID documents.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct Resolver;
