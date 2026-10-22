//! Hashing: fact identifiers. ADR 0035.

use lp_core::envelope::FactId;
use sha2::{Digest as _, Sha256};

/// SHA-256 of `bytes`.
#[must_use]
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// A SHA-256 computed over bytes as they stream past: for a file transfer,
/// where the whole content is never in memory at once.
#[derive(Debug, Clone, Default)]
pub struct Sha256Stream(Sha256);

impl Sha256Stream {
    /// An empty digest.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `bytes`.
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    /// The digest of everything added.
    #[must_use]
    pub fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

/// The identifier of a fact: SHA-256 of its payload bytes as received.
#[must_use]
pub fn fact_id(payload: &[u8]) -> FactId {
    FactId(sha256(payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_digest() {
        assert_eq!(
            fact_id(b"abc").to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut streamed = Sha256Stream::new();
        streamed.update(b"a");
        streamed.update(b"bc");
        assert_eq!(streamed.finish(), sha256(b"abc"));
    }
}
