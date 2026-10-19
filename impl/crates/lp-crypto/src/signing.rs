//! Ed25519 signing keys. Spec chapter 01; ADR 0027.
//!
//! A [`SigningKey`] is 32 seed bytes; its public half is an
//! [`lp_core::PublicKey`], which is the identifier of whatever the key stands
//! for (a person's master, self-signing or user-signing key, a device's
//! principal or binding key). Seeds come from the operating system through
//! `getrandom`. Every signature in the protocol is over a plain byte
//! concatenation (ADR 0035); the domain prefixes live with the facts that use
//! them.

use ed25519_dalek::Signer as _;
use ed25519_dalek::Verifier as _;
use lp_core::{Error, PublicKey};

/// Length of an Ed25519 signature.
pub const SIGNATURE_LEN: usize = 64;

/// An Ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature([u8; SIGNATURE_LEN]);

impl Signature {
    /// Wraps signature bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; SIGNATURE_LEN]) -> Self {
        Self(bytes)
    }

    /// Reads a signature from a slice.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidSignature`] if the slice is not 64 bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| Error::InvalidSignature("wrong length".to_owned()))
    }

    /// The signature bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; SIGNATURE_LEN] {
        &self.0
    }
}

impl core::fmt::Debug for Signature {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Signature({}…)", hex_prefix(&self.0))
    }
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// A signing key pair held by this device.
///
/// The `Debug` form shows only the public key.
#[derive(Clone)]
pub struct SigningKey {
    inner: ed25519_dalek::SigningKey,
    public: PublicKey,
}

impl SigningKey {
    /// Generates a key from operating-system randomness.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Randomness`] if the operating system refuses.
    pub fn generate() -> Result<Self, Error> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| Error::Randomness(e.to_string()))?;
        Ok(Self::from_seed(seed))
    }

    /// Rebuilds a key from its 32-byte seed.
    #[must_use]
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let inner = ed25519_dalek::SigningKey::from_bytes(&seed);
        let public = PublicKey::from_bytes(inner.verifying_key().to_bytes());
        Self { inner, public }
    }

    /// The seed, for the key file. Handle with care.
    #[must_use]
    pub fn seed(&self) -> [u8; 32] {
        self.inner.to_bytes()
    }

    /// The public key: the identifier.
    #[must_use]
    pub const fn public(&self) -> &PublicKey {
        &self.public
    }

    /// Signs a message. Ed25519 is deterministic: the same key and message
    /// always give the same signature.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> Signature {
        Signature(self.inner.sign(message).to_bytes())
    }
}

impl core::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SigningKey({})", self.public)
    }
}

/// Verifies `signature` over `message` with `public`.
///
/// # Errors
///
/// Returns [`Error::InvalidSignature`] if the key bytes are not a valid
/// Ed25519 point or the signature does not verify.
pub fn verify(public: &PublicKey, message: &[u8], signature: &Signature) -> Result<(), Error> {
    let key = ed25519_dalek::VerifyingKey::from_bytes(public.as_bytes())
        .map_err(|e| Error::InvalidSignature(format!("bad public key: {e}")))?;
    key.verify(message, &ed25519_dalek::Signature::from_bytes(&signature.0))
        .map_err(|_| Error::InvalidSignature("does not verify".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_sign_verify() {
        let key = SigningKey::generate().unwrap();
        let sig = key.sign(b"hello");
        verify(key.public(), b"hello", &sig).unwrap();
        assert!(verify(key.public(), b"hullo", &sig).is_err());
        let other = SigningKey::generate().unwrap();
        assert!(verify(other.public(), b"hello", &sig).is_err());
        assert_ne!(key.public(), other.public());
    }

    #[test]
    fn seed_round_trips_and_signatures_are_deterministic() {
        let key = SigningKey::generate().unwrap();
        let again = SigningKey::from_seed(key.seed());
        assert_eq!(key.public(), again.public());
        assert_eq!(key.sign(b"x"), again.sign(b"x"));
        assert!(key.public().to_multibase().starts_with("z6Mk"));
    }

    #[test]
    fn signature_slice_length_is_checked() {
        assert!(Signature::from_slice(&[0; 63]).is_err());
        assert!(Signature::from_slice(&[0; 64]).is_ok());
    }
}
