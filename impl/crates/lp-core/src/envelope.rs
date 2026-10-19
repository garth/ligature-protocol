//! The one container for every signed fact. Spec chapter 06 section 3 and
//! `schemas/envelope.schema.json` (ADR 0035).
//!
//! The payload is the encoded fact exactly as the signer produced it and is
//! never re-encoded. The signature is Ed25519 over the byte concatenation
//! `"ligature/fact/1" || type || payload`. A verifier checks the bytes as
//! received, resolves the signer to a trusted chain, then decodes the payload.
//! Signing and verifying live in `lp-identity`; this is the shape and the
//! byte string that is signed.

use core::fmt;

use serde::{Deserialize, Serialize};

use crate::PublicKey;
use crate::wire::{Bytes, TypeName};

/// Domain prefix every fact signature is made over.
pub const FACT_DOMAIN: &[u8] = b"ligature/fact/1";

/// A signed fact as it travels and is stored: byte-exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// Reverse-DNS type of the payload, naming its schema.
    #[serde(rename = "type")]
    pub fact_type: TypeName,
    /// The encoded fact, as transmitted.
    pub payload: Bytes,
    /// The key that signed.
    pub signer: PublicKey,
    /// Ed25519 signature over `FACT_DOMAIN || fact_type || payload`.
    pub signature: Bytes,
}

impl Envelope {
    /// The exact bytes the signature covers.
    #[must_use]
    pub fn signed_bytes(&self) -> Vec<u8> {
        signed_bytes(&self.fact_type, &self.payload.0)
    }
}

/// The bytes a signature over a fact of `fact_type` with `payload` covers.
#[must_use]
pub fn signed_bytes(fact_type: &TypeName, payload: &[u8]) -> Vec<u8> {
    let t = fact_type.as_str().as_bytes();
    let mut m = Vec::with_capacity(FACT_DOMAIN.len() + t.len() + payload.len());
    m.extend_from_slice(FACT_DOMAIN);
    m.extend_from_slice(t);
    m.extend_from_slice(payload);
    m
}

/// A fact's identifier: the SHA-256 of its payload bytes, the same on every
/// device. Rendered as 64 lowercase hex characters.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FactId(pub [u8; 32]);

impl FactId {
    /// The hex form.
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Parses the hex form.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Decode`] unless the text is 64 hex characters.
    pub fn from_hex(s: &str) -> Result<Self, crate::Error> {
        let bad = || crate::Error::Decode(format!("fact id {s:?}"));
        if s.len() != 64 {
            return Err(bad());
        }
        let mut out = [0u8; 32];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let pair = core::str::from_utf8(chunk).map_err(|_| bad())?;
            out[i] = u8::from_str_radix(pair, 16).map_err(|_| bad())?;
        }
        Ok(Self(out))
    }
}

impl fmt::Display for FactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for FactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FactId({}…)", &self.to_hex()[..12])
    }
}

impl Serialize for FactId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            s.serialize_str(&self.to_hex())
        } else {
            s.serialize_bytes(&self.0)
        }
    }
}

impl<'de> Deserialize<'de> for FactId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let b = Bytes::deserialize(d)?;
        if let Ok(arr) = <[u8; 32]>::try_from(b.0.as_slice()) {
            return Ok(Self(arr));
        }
        Err(serde::de::Error::custom("fact id must be 32 bytes"))
    }
}
