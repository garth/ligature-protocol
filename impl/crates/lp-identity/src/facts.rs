//! Signing, verifying and decoding facts. Spec chapter 06 section 3; ADR 0035.
//!
//! An [`Envelope`] is signed over `"ligature/fact/1" || type || payload` and
//! verified over the bytes exactly as received; only then is the payload
//! decoded. The functions here know the two levels: the signature on the
//! envelope, and the chain that says why the signer was entitled to sign.

use lp_core::envelope::{Envelope, FactId};
use lp_core::facts::{
    FactType, PersonFact, RevocationFact, RevocationKind, Role, RoleFact, SubordinateKey,
};
use lp_core::wire::{Bytes, Timestamp, TypeName};
use lp_core::{Error, PublicKey};
use lp_crypto::hash;
use lp_crypto::signing::{Signature, SigningKey, verify};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::cross_signing::{PersonKeys, Purpose, verify_subkey};

/// Encodes `fact` as CBOR and signs it with `key` into an envelope.
///
/// # Errors
///
/// Returns [`Error::Decode`] if the fact cannot be encoded.
pub fn sign<T: Serialize + FactType>(key: &SigningKey, fact: &T) -> Result<Envelope, Error> {
    sign_as(key, &T::type_name(), fact)
}

/// Like [`sign`] but with an explicit type name. Exists so tests can produce
/// an envelope whose type does not match its payload.
///
/// # Errors
///
/// Returns [`Error::Decode`] if the fact cannot be encoded.
pub fn sign_as<T: Serialize>(
    key: &SigningKey,
    fact_type: &TypeName,
    fact: &T,
) -> Result<Envelope, Error> {
    let mut payload = Vec::new();
    ciborium::into_writer(fact, &mut payload).map_err(|e| Error::Decode(format!("encode: {e}")))?;
    Ok(sign_payload(key, fact_type, payload))
}

/// Signs already-encoded payload bytes.
#[must_use]
pub fn sign_payload(key: &SigningKey, fact_type: &TypeName, payload: Vec<u8>) -> Envelope {
    let signature = key.sign(&lp_core::envelope::signed_bytes(fact_type, &payload));
    Envelope {
        fact_type: fact_type.clone(),
        payload: Bytes(payload),
        signer: *key.public(),
        signature: Bytes(signature.as_bytes().to_vec()),
    }
}

/// Checks the envelope's signature over its bytes as received. Says nothing
/// about whether the signer was entitled to sign.
///
/// # Errors
///
/// Returns [`Error::InvalidSignature`] if the signature is malformed or fails.
pub fn verify_signature(envelope: &Envelope) -> Result<(), Error> {
    let signature = Signature::from_slice(&envelope.signature.0)?;
    verify(&envelope.signer, &envelope.signed_bytes(), &signature)
}

/// Decodes the payload as `T`, after checking the type name matches.
/// Forgiving: any valid CBOR, unknown fields ignored.
///
/// # Errors
///
/// Returns [`Error::Decode`] on a type mismatch or malformed payload.
pub fn decode<T: DeserializeOwned + FactType>(envelope: &Envelope) -> Result<T, Error> {
    if envelope.fact_type.as_str() != T::TYPE {
        return Err(Error::Decode(format!(
            "expected {} but the envelope is {}",
            T::TYPE,
            envelope.fact_type
        )));
    }
    ciborium::from_reader(envelope.payload.0.as_slice())
        .map_err(|e| Error::Decode(format!("{}: {e}", T::TYPE)))
}

/// The fact identifier of an envelope.
#[must_use]
pub fn id(envelope: &Envelope) -> FactId {
    hash::fact_id(&envelope.payload.0)
}

/// Builds and signs a person fact from the person's keys. Signed by the
/// self-signing key, as chapter 06 requires.
///
/// # Errors
///
/// Returns [`Error::Decode`] if encoding fails.
pub fn person_fact(
    keys: &PersonKeys,
    display_name: &str,
    version: u64,
    updated_at: Timestamp,
) -> Result<Envelope, Error> {
    let fact = PersonFact {
        master_key: *keys.person(),
        display_name: display_name.to_owned(),
        self_signing_key: SubordinateKey {
            key: *keys.self_signing.public(),
            master_signature: Bytes(keys.self_signing_signature.as_bytes().to_vec()),
        },
        user_signing_key: SubordinateKey {
            key: *keys.user_signing.public(),
            master_signature: Bytes(keys.user_signing_signature.as_bytes().to_vec()),
        },
        version,
        updated_at,
    };
    sign(&keys.self_signing, &fact)
}

/// Verifies a person fact completely: envelope signature, signer is the
/// fact's own self-signing key, and both subordinate keys are endorsed by the
/// master key. This is how a master key is learned in the ceremony; the caller
/// still decides whether it trusts that master key.
///
/// # Errors
///
/// Returns the first failure: [`Error::InvalidSignature`], [`Error::Decode`]
/// or [`Error::Chain`].
pub fn verify_person(envelope: &Envelope) -> Result<PersonFact, Error> {
    verify_signature(envelope)?;
    let fact: PersonFact = decode(envelope)?;
    if envelope.signer != fact.self_signing_key.key {
        return Err(Error::Chain(
            "person fact not signed by its self-signing key".to_owned(),
        ));
    }
    check_subkey(
        &fact.master_key,
        Purpose::SelfSigning,
        &fact.self_signing_key,
    )?;
    check_subkey(
        &fact.master_key,
        Purpose::UserSigning,
        &fact.user_signing_key,
    )?;
    Ok(fact)
}

fn check_subkey(master: &PublicKey, purpose: Purpose, sub: &SubordinateKey) -> Result<(), Error> {
    let sig = Signature::from_slice(&sub.master_signature.0)?;
    verify_subkey(master, purpose, &sub.key, &sig).map_err(|_| {
        Error::Chain(format!(
            "{} key not endorsed by the master key",
            purpose.as_str()
        ))
    })
}

/// Builds and signs a role fact. `binding` and `principal` are signed by the
/// self-signing key, `person` by the user-signing key.
///
/// # Errors
///
/// Returns [`Error::Decode`] if encoding fails.
pub fn role_fact(
    keys: &PersonKeys,
    role: Role,
    subject: PublicKey,
    introducer: bool,
    signed_at: Timestamp,
) -> Result<Envelope, Error> {
    let fact = RoleFact {
        subject,
        role,
        person: *keys.person(),
        introducer: introducer && role == Role::Binding,
        signed_at,
    };
    let signer = match role {
        Role::Binding | Role::Principal => &keys.self_signing,
        Role::Person => &keys.user_signing,
    };
    sign(signer, &fact)
}

/// Verifies a role fact against a verified person fact: envelope signature,
/// the fact names that person, and the signer is the subordinate key the role
/// requires. The person fact is the chain to the master key.
///
/// # Errors
///
/// Returns the first failure: [`Error::InvalidSignature`], [`Error::Decode`]
/// or [`Error::Chain`].
pub fn verify_role(envelope: &Envelope, person: &PersonFact) -> Result<RoleFact, Error> {
    verify_signature(envelope)?;
    let fact: RoleFact = decode(envelope)?;
    if fact.person != person.master_key {
        return Err(Error::Chain(
            "role fact names a different person".to_owned(),
        ));
    }
    let expected = match fact.role {
        Role::Binding | Role::Principal => &person.self_signing_key.key,
        Role::Person => &person.user_signing_key.key,
    };
    if envelope.signer != *expected {
        return Err(Error::Chain(format!(
            "role {:?} must be signed by the person's {} key",
            fact.role,
            match fact.role {
                Role::Person => "user-signing",
                Role::Binding | Role::Principal => "self-signing",
            }
        )));
    }
    Ok(fact)
}

/// Builds and signs a revocation or abandonment. Signed by the self-signing
/// key, as chapter 06 requires.
///
/// # Errors
///
/// Returns [`Error::Decode`] if encoding fails.
pub fn revocation_fact(
    keys: &PersonKeys,
    subject: PublicKey,
    kind: RevocationKind,
    revoked_at: Timestamp,
) -> Result<Envelope, Error> {
    let fact = RevocationFact {
        subject,
        kind,
        person: *keys.person(),
        revoked_at,
    };
    sign(&keys.self_signing, &fact)
}

/// Verifies a revocation against a verified person fact: envelope signature,
/// the fact names that person, and the signer is the person's self-signing
/// key.
///
/// # Errors
///
/// Returns the first failure: [`Error::InvalidSignature`], [`Error::Decode`]
/// or [`Error::Chain`].
pub fn verify_revocation(
    envelope: &Envelope,
    person: &PersonFact,
) -> Result<RevocationFact, Error> {
    verify_signature(envelope)?;
    let fact: RevocationFact = decode(envelope)?;
    if fact.person != person.master_key {
        return Err(Error::Chain(
            "revocation names a different person".to_owned(),
        ));
    }
    if envelope.signer != person.self_signing_key.key {
        return Err(Error::Chain(
            "revocation must be signed by the person's self-signing key".to_owned(),
        ));
    }
    Ok(fact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        Timestamp::from_unix(1_760_000_000).unwrap()
    }

    #[test]
    fn person_fact_round_trips_and_verifies() {
        let keys = PersonKeys::generate().unwrap();
        let env = person_fact(&keys, "Alice", 1, now()).unwrap();
        let fact = verify_person(&env).unwrap();
        assert_eq!(fact.display_name, "Alice");
        assert_eq!(&fact.master_key, keys.person());
        assert_eq!(id(&env).to_hex().len(), 64);
    }

    #[test]
    fn a_flipped_payload_byte_fails_the_signature() {
        let keys = PersonKeys::generate().unwrap();
        let mut env = person_fact(&keys, "Alice", 1, now()).unwrap();
        env.payload.0[10] ^= 1;
        assert!(matches!(
            verify_person(&env),
            Err(Error::InvalidSignature(_))
        ));
    }

    #[test]
    fn a_relabelled_type_fails_the_signature() {
        let keys = PersonKeys::generate().unwrap();
        let mut env = person_fact(&keys, "Alice", 1, now()).unwrap();
        env.fact_type = TypeName::new(lp_core::facts::ROLE_TYPE);
        assert!(matches!(
            verify_signature(&env),
            Err(Error::InvalidSignature(_))
        ));
    }

    #[test]
    fn a_person_fact_signed_by_the_wrong_key_fails_the_chain() {
        let keys = PersonKeys::generate().unwrap();
        let mallory = SigningKey::generate().unwrap();
        let fact: PersonFact = decode(&person_fact(&keys, "Alice", 1, now()).unwrap()).unwrap();
        let env = sign(&mallory, &fact).unwrap();
        assert!(matches!(verify_person(&env), Err(Error::Chain(_))));
    }

    #[test]
    fn role_facts_verify_against_the_person_and_reject_outsiders() {
        let keys = PersonKeys::generate().unwrap();
        let person = verify_person(&person_fact(&keys, "Alice", 1, now()).unwrap()).unwrap();
        let laptop = SigningKey::generate().unwrap();
        let env = role_fact(&keys, Role::Binding, *laptop.public(), true, now()).unwrap();
        let fact = verify_role(&env, &person).unwrap();
        assert!(fact.introducer);
        assert_eq!(fact.subject, *laptop.public());

        let principal = role_fact(&keys, Role::Principal, *laptop.public(), true, now()).unwrap();
        assert!(
            !verify_role(&principal, &person).unwrap().introducer,
            "introducer only for bindings"
        );

        let other = PersonKeys::generate().unwrap();
        let known = role_fact(&keys, Role::Person, *other.person(), false, now()).unwrap();
        assert_eq!(known.signer, *keys.user_signing.public());
        verify_role(&known, &person).unwrap();

        let mallory = SigningKey::generate().unwrap();
        let forged = sign(&mallory, &fact).unwrap();
        verify_signature(&forged).unwrap();
        assert!(matches!(
            verify_role(&forged, &person),
            Err(Error::Chain(_))
        ));

        let wrong_key = sign(&keys.user_signing, &fact).unwrap();
        assert!(
            matches!(verify_role(&wrong_key, &person), Err(Error::Chain(_))),
            "binding by user-signing key"
        );

        let other_person = verify_person(&person_fact(&other, "Bob", 1, now()).unwrap()).unwrap();
        assert!(matches!(
            verify_role(&env, &other_person),
            Err(Error::Chain(_))
        ));
    }

    #[test]
    fn revocations_verify_against_the_person() {
        let keys = PersonKeys::generate().unwrap();
        let person = verify_person(&person_fact(&keys, "Alice", 1, now()).unwrap()).unwrap();
        let laptop = SigningKey::generate().unwrap();
        let env =
            revocation_fact(&keys, *laptop.public(), RevocationKind::Revocation, now()).unwrap();
        let fact = verify_revocation(&env, &person).unwrap();
        assert_eq!(fact.subject, *laptop.public());
        assert_eq!(fact.kind, RevocationKind::Revocation);
        let abandon =
            revocation_fact(&keys, *keys.person(), RevocationKind::Abandonment, now()).unwrap();
        assert_eq!(
            verify_revocation(&abandon, &person).unwrap().kind,
            RevocationKind::Abandonment
        );
        let by_user_key = sign(&keys.user_signing, &fact).unwrap();
        assert!(matches!(
            verify_revocation(&by_user_key, &person),
            Err(Error::Chain(_))
        ));
    }

    #[test]
    fn decode_checks_the_type_name() {
        let keys = PersonKeys::generate().unwrap();
        let env = person_fact(&keys, "Alice", 1, now()).unwrap();
        assert!(matches!(decode::<RoleFact>(&env), Err(Error::Decode(_))));
    }
}
