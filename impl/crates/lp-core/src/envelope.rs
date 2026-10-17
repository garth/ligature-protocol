//! The one container for every signed fact. Spec chapter 06 section 3 and
//! `schemas/envelope.schema.json` (ADR 0035).
//!
//! The payload is the encoded fact exactly as the signer produced it and is
//! never re-encoded. The signature is Ed25519 over the byte concatenation
//! `"ligature/fact/1" || type || payload`. A verifier checks the bytes as
//! received, resolves the signer to a trusted chain, then decodes the payload.

/// Domain prefix every fact signature is made over.
pub const FACT_DOMAIN: &[u8] = b"ligature/fact/1";

/// A signed fact as it travels and is stored: byte-exact.
///
/// Not yet implemented beyond the shape.
#[derive(Debug, Clone)]
pub struct Envelope {
    /// Reverse-DNS type of the payload, naming its schema.
    pub fact_type: crate::capability::CapabilityType,
    /// The encoded fact, as transmitted.
    pub payload: Vec<u8>,
    /// The key that signed, in multibase form.
    pub signer: String,
    /// Ed25519 signature over `FACT_DOMAIN || fact_type || payload`.
    pub signature: Vec<u8>,
}
