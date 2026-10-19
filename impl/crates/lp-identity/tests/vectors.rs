//! Conformance vectors for subordinate keys and facts: generated on demand
//! from the test keys, checked on every run (`vectors/README.md`).

use std::collections::HashMap;
use std::path::Path;

use lp_core::envelope::Envelope;
use lp_core::facts::{PERSON_TYPE, PersonFact, REVOCATION_TYPE, ROLE_TYPE, Role, RoleFact};
use lp_core::wire::{Bytes, Timestamp, TypeName};
use lp_core::{Error, PublicKey};
use lp_crypto::signing::{Signature, SigningKey};
use lp_identity::cross_signing::{PersonKeys, Purpose, sign_subkey, verify_subkey};
use lp_identity::facts;
use lp_vectors::{
    EnvelopeHex, Expect, FORMAT, FactVector, Hex, KeyFile, SubkeyVector, load_dir, load_file,
    may_write, vectors_dir, write_file,
};

struct Keys {
    by_name: HashMap<String, SigningKey>,
}

impl Keys {
    fn load() -> Self {
        let file: KeyFile = load_file(&vectors_dir().join("keys.json")).expect("keys.json");
        let by_name = file
            .keys
            .iter()
            .map(|k| {
                let seed: [u8; 32] = k.seed.0.clone().try_into().expect("seed");
                (k.name.clone(), SigningKey::from_seed(seed))
            })
            .collect();
        Self { by_name }
    }

    fn get(&self, name: &str) -> &SigningKey {
        self.by_name
            .get(name)
            .unwrap_or_else(|| panic!("no test key {name}"))
    }

    fn by_public(&self, public: &PublicKey) -> Option<&SigningKey> {
        self.by_name.values().find(|k| k.public() == public)
    }

    fn alice(&self) -> PersonKeys {
        PersonKeys::from_keys(
            self.get("alice-master").clone(),
            self.get("alice-self").clone(),
            self.get("alice-user").clone(),
        )
    }
}

fn at() -> Timestamp {
    Timestamp::from_unix(1_760_000_000).unwrap()
}

fn hex_envelope(e: &Envelope) -> EnvelopeHex {
    EnvelopeHex {
        fact_type: e.fact_type.to_string(),
        payload: Hex(e.payload.0.clone()),
        signer: e.signer.to_string(),
        signature: Hex(e.signature.0.clone()),
    }
}

fn envelope(h: &EnvelopeHex) -> Envelope {
    Envelope {
        fact_type: TypeName::new(&h.fact_type),
        payload: Bytes(h.payload.0.clone()),
        signer: h.signer.parse().expect("signer"),
        signature: Bytes(h.signature.0.clone()),
    }
}

fn fact_vector<T: serde::Serialize>(
    description: &str,
    keys: &[&str],
    input: &T,
    env: &Envelope,
    chain: Option<&Envelope>,
    expect: Expect,
) -> FactVector {
    FactVector {
        format: FORMAT.to_owned(),
        description: description.to_owned(),
        keys: keys.iter().map(|k| (*k).to_owned()).collect(),
        input: serde_json::to_value(input).unwrap(),
        fact_type: env.fact_type.to_string(),
        payload: Hex(env.payload.0.clone()),
        signer: env.signer.to_string(),
        signature: Hex(env.signature.0.clone()),
        fact_id: Hex(facts::id(env).0.to_vec()),
        chain: chain.map(hex_envelope),
        expect,
    }
}

fn invalid(reason: &str) -> Expect {
    Expect::Invalid {
        reason: reason.to_owned(),
    }
}

fn write_if_missing<T: serde::Serialize>(dir: &Path, name: &str, make: impl FnOnce() -> T) {
    let path = dir.join(name);
    if !path.exists() {
        write_file(&path, &make()).expect("write vector");
    }
}

const ALICE: [&str; 3] = ["alice-master", "alice-self", "alice-user"];

#[test]
fn generate_vectors_on_demand() {
    if !may_write() {
        return;
    }
    let keys = Keys::load();
    let alice = keys.alice();
    let keys_dir = vectors_dir().join("keys");
    let facts_dir = vectors_dir().join("facts");

    let subkey =
        |desc: &str, purpose: Purpose, key: &SigningKey, sig: Signature, expect| SubkeyVector {
            format: FORMAT.to_owned(),
            description: desc.to_owned(),
            keys: ALICE.iter().map(|k| (*k).to_owned()).collect(),
            master: alice.person().to_string(),
            purpose: purpose.as_str().to_owned(),
            key: key.public().to_string(),
            signature: Hex(sig.as_bytes().to_vec()),
            expect,
        };
    write_if_missing(&keys_dir, "subkey-self-signing.json", || {
        subkey(
            "The master key endorses alice-self as the self-signing key.",
            Purpose::SelfSigning,
            &alice.self_signing,
            alice.self_signing_signature,
            Expect::Valid,
        )
    });
    write_if_missing(&keys_dir, "subkey-user-signing.json", || {
        subkey(
            "The master key endorses alice-user as the user-signing key.",
            Purpose::UserSigning,
            &alice.user_signing,
            alice.user_signing_signature,
            Expect::Valid,
        )
    });
    write_if_missing(&keys_dir, "subkey-purpose-swapped.json", || {
        subkey(
            "The self-signing endorsement presented as a user-signing one: the purpose is part of the signed bytes.",
            Purpose::UserSigning,
            &alice.self_signing,
            alice.self_signing_signature,
            invalid("purpose differs from the one signed"),
        )
    });

    let person_env = facts::person_fact(&alice, "Alice", 1, at()).unwrap();
    let person: PersonFact = facts::decode(&person_env).unwrap();
    write_if_missing(&facts_dir, "person-alice.json", || {
        fact_vector(
            "Alice's person fact, signed by her self-signing key.",
            &ALICE,
            &person,
            &person_env,
            None,
            Expect::Valid,
        )
    });
    write_if_missing(&facts_dir, "person-payload-tampered.json", || {
        let mut env = person_env.clone();
        env.payload.0[20] ^= 0x01;
        fact_vector(
            "Alice's person fact with one payload byte flipped: the signature fails.",
            &ALICE,
            &person,
            &env,
            None,
            invalid("signature does not verify over the bytes as received"),
        )
    });
    write_if_missing(&facts_dir, "person-type-relabelled.json", || {
        let mut env = person_env.clone();
        env.fact_type = TypeName::new(REVOCATION_TYPE);
        fact_vector(
            "Alice's person fact in an envelope claiming another type: the type is part of the signed bytes.",
            &ALICE,
            &person,
            &env,
            None,
            invalid("signature does not verify: the type was changed"),
        )
    });
    write_if_missing(&facts_dir, "person-signed-by-outsider.json", || {
        let env = facts::sign(keys.get("mallory"), &person).unwrap();
        fact_vector(
            "Alice's person fact signed by mallory: the signature verifies but the signer is not the fact's self-signing key.",
            &["alice-master", "alice-self", "alice-user", "mallory"],
            &person,
            &env,
            None,
            invalid("chain: signer is not the self-signing key named in the fact"),
        )
    });

    let laptop = keys.get("laptop-binding");
    let principal = keys.get("laptop-principal");
    let binding_env =
        facts::role_fact(&alice, Role::Binding, *laptop.public(), true, at()).unwrap();
    let binding: RoleFact = facts::decode(&binding_env).unwrap();
    write_if_missing(&facts_dir, "role-binding-introducer.json", || {
        fact_vector(
            "laptop-binding is Alice's account on the laptop and an introducer; signed by alice-self.",
            &["alice-master", "alice-self", "alice-user", "laptop-binding"],
            &binding,
            &binding_env,
            Some(&person_env),
            Expect::Valid,
        )
    });
    write_if_missing(&facts_dir, "role-principal.json", || {
        let env =
            facts::role_fact(&alice, Role::Principal, *principal.public(), false, at()).unwrap();
        let fact: RoleFact = facts::decode(&env).unwrap();
        fact_vector(
            "laptop-principal is a device Alice uses; signed by alice-self.",
            &[
                "alice-master",
                "alice-self",
                "alice-user",
                "laptop-principal",
            ],
            &fact,
            &env,
            Some(&person_env),
            Expect::Valid,
        )
    });
    write_if_missing(&facts_dir, "role-person-known.json", || {
        let env = facts::role_fact(
            &alice,
            Role::Person,
            *keys.get("mallory").public(),
            false,
            at(),
        )
        .unwrap();
        let fact: RoleFact = facts::decode(&env).unwrap();
        fact_vector(
            "Alice knows the person whose master key is mallory's key; signed by alice-user.",
            &["alice-master", "alice-self", "alice-user", "mallory"],
            &fact,
            &env,
            Some(&person_env),
            Expect::Valid,
        )
    });
    write_if_missing(&facts_dir, "role-signed-by-outsider.json", || {
        let env = facts::sign(keys.get("mallory"), &binding).unwrap();
        fact_vector(
            "The binding role fact signed by mallory: the signature verifies but mallory is not in Alice's chain.",
            &[
                "alice-master",
                "alice-self",
                "alice-user",
                "laptop-binding",
                "mallory",
            ],
            &binding,
            &env,
            Some(&person_env),
            invalid("chain: signer is not Alice's self-signing key"),
        )
    });
    write_if_missing(&facts_dir, "role-binding-by-user-signing-key.json", || {
        let env = facts::sign(&alice.user_signing, &binding).unwrap();
        fact_vector(
            "A binding role signed by the user-signing key: bindings and principals need the self-signing key.",
            &ALICE,
            &binding,
            &env,
            Some(&person_env),
            invalid("chain: wrong subordinate key for the role"),
        )
    });
}

/// What the reference does with a fact vector: the outcome, and on success the
/// decoded fact as JSON for comparison with `input`.
fn check_fact(v: &FactVector) -> Result<serde_json::Value, Error> {
    let env = envelope(&EnvelopeHex {
        fact_type: v.fact_type.clone(),
        payload: v.payload.clone(),
        signer: v.signer.clone(),
        signature: v.signature.clone(),
    });
    match v.fact_type.as_str() {
        PERSON_TYPE => {
            let fact = facts::verify_person(&env)?;
            Ok(serde_json::to_value(fact).unwrap())
        }
        ROLE_TYPE => {
            let chain = v
                .chain
                .as_ref()
                .expect("role vectors carry the person fact");
            let person = facts::verify_person(&envelope(chain))?;
            let fact = facts::verify_role(&env, &person)?;
            Ok(serde_json::to_value(fact).unwrap())
        }
        // A relabelled envelope: verify the signature as any receiver would first.
        _ => {
            facts::verify_signature(&env)?;
            Err(Error::Decode(format!("no checker for {}", v.fact_type)))
        }
    }
}

#[test]
fn every_subkey_vector_behaves_as_expected() {
    if may_write() {
        return;
    }
    let keys = Keys::load();
    let vectors: Vec<(_, SubkeyVector)> = load_dir(&vectors_dir().join("keys")).expect("keys/");
    assert!(
        !vectors.is_empty(),
        "no subkey vectors; run with LP_WRITE_VECTORS=1"
    );
    for (path, v) in vectors {
        let name = path.display();
        assert_eq!(v.format, FORMAT, "{name}");
        let master: PublicKey = v.master.parse().unwrap();
        let key: PublicKey = v.key.parse().unwrap();
        let purpose = match v.purpose.as_str() {
            "self-signing" => Purpose::SelfSigning,
            "user-signing" => Purpose::UserSigning,
            p => panic!("{name}: purpose {p}"),
        };
        let sig = Signature::from_slice(&v.signature.0).unwrap();
        let outcome = verify_subkey(&master, purpose, &key, &sig);
        match &v.expect {
            Expect::Valid => {
                outcome.unwrap_or_else(|e| panic!("{name}: {e}"));
                let signer = keys.by_public(&master).expect("master seed");
                assert_eq!(sign_subkey(signer, purpose, &key), sig, "{name}: reproduce");
            }
            Expect::Invalid { reason } => {
                assert!(outcome.is_err(), "{name}: expected invalid ({reason})");
            }
        }
    }
}

#[test]
fn every_fact_vector_behaves_as_expected() {
    if may_write() {
        return;
    }
    let keys = Keys::load();
    let vectors: Vec<(_, FactVector)> = load_dir(&vectors_dir().join("facts")).expect("facts/");
    assert!(
        !vectors.is_empty(),
        "no fact vectors; run with LP_WRITE_VECTORS=1"
    );
    for (path, v) in vectors {
        let name = path.display();
        assert_eq!(v.format, FORMAT, "{name}");
        assert_eq!(
            lp_crypto::hash::sha256(&v.payload.0).to_vec(),
            v.fact_id.0,
            "{name}: fact id"
        );
        let outcome = check_fact(&v);
        match &v.expect {
            Expect::Valid => {
                let decoded = outcome.unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_eq!(decoded, v.input, "{name}: decoded payload equals input");
                let signer: PublicKey = v.signer.parse().unwrap();
                let key = keys.by_public(&signer).expect("signer seed");
                let again =
                    facts::sign_payload(key, &TypeName::new(&v.fact_type), v.payload.0.clone());
                assert_eq!(
                    again.signature.0, v.signature.0,
                    "{name}: reproduce signature"
                );
                // The reference also reproduces the bytes; other implementations need not.
                let reencoded = if v.fact_type == PERSON_TYPE {
                    encode(&serde_json::from_value::<PersonFact>(v.input.clone()).unwrap())
                } else {
                    encode(&serde_json::from_value::<RoleFact>(v.input.clone()).unwrap())
                };
                assert_eq!(
                    reencoded, v.payload.0,
                    "{name}: reference re-encodes identically"
                );
            }
            Expect::Invalid { reason } => {
                assert!(outcome.is_err(), "{name}: expected invalid ({reason})");
            }
        }
    }
}

fn encode<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).unwrap();
    out
}
