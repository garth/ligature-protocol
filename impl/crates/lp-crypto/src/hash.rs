//! Hashing: fact identifiers. ADR 0035.

use lp_core::envelope::FactId;
use sha2::{Digest as _, Sha256};

/// SHA-256 of `bytes`.
#[must_use]
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
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
    }
}
