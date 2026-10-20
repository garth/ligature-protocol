//! The ceremony messages validate against `schemas/pairing.schema.json`.

use std::collections::HashMap;
use std::path::Path;

use jsonschema::{Retrieve, Uri};
use lp_core::wire::{Bytes, Timestamp, TypeName};
use lp_crypto::signing::SigningKey;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::pairing::{Abort, Confirmed, Done, Grant, Hello, Newcomer, grant_roles, types};

struct Local(HashMap<String, serde_json::Value>);

impl Local {
    fn load() -> Self {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../schemas");
        let mut m = HashMap::new();
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "json") {
                let v: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
                m.insert(v["$id"].as_str().unwrap().to_owned(), v);
            }
        }
        Self(m)
    }
}

impl Retrieve for Local {
    fn retrieve(
        &self,
        uri: &Uri<String>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        self.0
            .get(uri.as_str())
            .cloned()
            .ok_or_else(|| format!("no schema {uri}").into())
    }
}

fn check<T: serde::Serialize + serde::de::DeserializeOwned>(value: &T) {
    let schemas = Local::load();
    let schema = schemas.0["https://ligatureprotocol.org/schemas/pairing.schema.json"].clone();
    let validator = jsonschema::options()
        .with_retriever(Local::load())
        .build(&schema)
        .unwrap();
    let mut cbor = Vec::new();
    ciborium::into_writer(value, &mut cbor).unwrap();
    let back: T = ciborium::from_reader(cbor.as_slice()).unwrap();
    let json = serde_json::to_value(&back).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&json)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:?}\n{json}");
}

#[test]
fn ceremony_messages_validate() {
    let at = Timestamp::from_unix(1_760_000_000).unwrap();
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at).unwrap();
    let mut n = Newcomer::new(
        SigningKey::generate().unwrap(),
        SigningKey::generate().unwrap(),
        "laptop",
        "linux",
        at,
    )
    .unwrap();
    let offer = n.offer(vec!["127.0.0.1:1".parse().unwrap()]);
    let hello = Hello {
        message_type: TypeName::new(types::HELLO),
        code: offer.code.clone(),
    };
    check(&hello);
    let ready = n.on_hello(*alice.self_signing.public(), &hello).unwrap();
    check(&ready);
    check(&Confirmed {
        message_type: TypeName::new(types::CONFIRMED),
    });
    check(&Grant {
        message_type: TypeName::new(types::GRANT),
        person,
        roles: grant_roles(&alice.self_signing, *alice.person(), &offer, true, at).unwrap(),
        self_signing: Some(Bytes(alice.self_signing.seed().to_vec())),
        gossip: vec![],
    });
    check(&Done {
        message_type: TypeName::new(types::DONE),
    });
    check(&Abort::new("codes differ"));
}
