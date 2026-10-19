//! Keys and role signatures. Spec chapters 01 and 02; ADR 0031.
//!
//! Follows the Matrix model with a role attached: a master key signs a
//! self-signing key (used by introducers to sign bindings and principals) and
//! a user-signing key (used to sign known persons and group acceptances). All
//! three always exist. A subordinate key's signature by the master key is over
//! the byte concatenation `"ligature/subkey/1" || purpose || key bytes`
//! (ADR 0035); the role facts themselves are envelopes (see [`crate::facts`]).

use lp_core::{Error, PublicKey};
use lp_crypto::signing::{Signature, SigningKey, verify};

/// Domain prefix of a master key's signature over a subordinate key.
pub const SUBKEY_DOMAIN: &[u8] = b"ligature/subkey/1";

/// The role a ceremony signature asserts about a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// My account on a device: member of my device set.
    Binding,
    /// A device I use: I may request its device-scoped capabilities.
    Principal,
    /// Another person's master key that I have verified in person.
    Person,
}

/// Which subordinate key a master signature is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// Held by introducers; signs bindings and principals.
    SelfSigning,
    /// Signs known persons and group acceptances.
    UserSigning,
}

impl Purpose {
    /// The purpose string that goes into the signed bytes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SelfSigning => "self-signing",
            Self::UserSigning => "user-signing",
        }
    }
}

/// The bytes a master key signs to endorse a subordinate key.
#[must_use]
pub fn subkey_message(purpose: Purpose, key: &PublicKey) -> Vec<u8> {
    let mut m = Vec::with_capacity(SUBKEY_DOMAIN.len() + 12 + 32);
    m.extend_from_slice(SUBKEY_DOMAIN);
    m.extend_from_slice(purpose.as_str().as_bytes());
    m.extend_from_slice(key.as_bytes());
    m
}

/// Signs a subordinate key with the master key.
#[must_use]
pub fn sign_subkey(master: &SigningKey, purpose: Purpose, key: &PublicKey) -> Signature {
    master.sign(&subkey_message(purpose, key))
}

/// Verifies a master key's endorsement of a subordinate key.
///
/// # Errors
///
/// Returns [`Error::InvalidSignature`] if it does not verify.
pub fn verify_subkey(
    master: &PublicKey,
    purpose: Purpose,
    key: &PublicKey,
    signature: &Signature,
) -> Result<(), Error> {
    verify(master, &subkey_message(purpose, key), signature)
}

/// A person's key material: the master key and both subordinate keys, with the
/// master's signatures over them. For milestone 1 all three private keys live
/// on the first device.
#[derive(Debug, Clone)]
pub struct PersonKeys {
    /// The person's identity.
    pub master: SigningKey,
    /// Signs bindings and principals.
    pub self_signing: SigningKey,
    /// Signs known persons and group acceptances.
    pub user_signing: SigningKey,
    /// Master signature over the self-signing key.
    pub self_signing_signature: Signature,
    /// Master signature over the user-signing key.
    pub user_signing_signature: Signature,
}

impl PersonKeys {
    /// Creates a new person: three fresh keys, the master endorsing the other two.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Randomness`] if the operating system refuses.
    pub fn generate() -> Result<Self, Error> {
        Ok(Self::from_keys(
            SigningKey::generate()?,
            SigningKey::generate()?,
            SigningKey::generate()?,
        ))
    }

    /// Assembles a person from existing keys, producing the endorsements.
    #[must_use]
    pub fn from_keys(
        master: SigningKey,
        self_signing: SigningKey,
        user_signing: SigningKey,
    ) -> Self {
        let self_signing_signature =
            sign_subkey(&master, Purpose::SelfSigning, self_signing.public());
        let user_signing_signature =
            sign_subkey(&master, Purpose::UserSigning, user_signing.public());
        Self {
            master,
            self_signing,
            user_signing,
            self_signing_signature,
            user_signing_signature,
        }
    }

    /// The person's identifier.
    #[must_use]
    pub const fn person(&self) -> &PublicKey {
        self.master.public()
    }

    /// Checks both endorsements against the master key.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidSignature`] if either fails.
    pub fn verify(&self) -> Result<(), Error> {
        verify_subkey(
            self.person(),
            Purpose::SelfSigning,
            self.self_signing.public(),
            &self.self_signing_signature,
        )?;
        verify_subkey(
            self.person(),
            Purpose::UserSigning,
            self.user_signing.public(),
            &self.user_signing_signature,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_person_has_three_keys_endorsed_by_the_master() {
        let p = PersonKeys::generate().unwrap();
        p.verify().unwrap();
        assert_ne!(p.master.public(), p.self_signing.public());
        assert_ne!(p.self_signing.public(), p.user_signing.public());
        verify_subkey(
            p.person(),
            Purpose::SelfSigning,
            p.self_signing.public(),
            &p.self_signing_signature,
        )
        .unwrap();
    }

    #[test]
    fn a_tampered_or_swapped_subordinate_key_fails() {
        let p = PersonKeys::generate().unwrap();
        let other = SigningKey::generate().unwrap();
        assert!(
            verify_subkey(
                p.person(),
                Purpose::SelfSigning,
                other.public(),
                &p.self_signing_signature
            )
            .is_err(),
            "different key"
        );
        assert!(
            verify_subkey(
                p.person(),
                Purpose::UserSigning,
                p.self_signing.public(),
                &p.self_signing_signature
            )
            .is_err(),
            "purpose swapped"
        );
        assert!(
            verify_subkey(
                other.public(),
                Purpose::SelfSigning,
                p.self_signing.public(),
                &p.self_signing_signature
            )
            .is_err(),
            "different master"
        );
    }

    #[test]
    fn the_signed_bytes_are_the_documented_concatenation() {
        let key = PublicKey::from_bytes([9; 32]);
        let m = subkey_message(Purpose::UserSigning, &key);
        let mut expected = b"ligature/subkey/1user-signing".to_vec();
        expected.extend_from_slice(&[9; 32]);
        assert_eq!(m, expected);
    }
}
