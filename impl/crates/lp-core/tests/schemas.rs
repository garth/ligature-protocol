//! Every message and fact the reference produces, round-tripped through CBOR
//! and validated as JSON against the schemas under `schemas/` (ADR 0020,
//! 0025, 0036). Cross-file references resolve to the local files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use jsonschema::{Retrieve, Uri, Validator};
use lp_core::PublicKey;
use lp_core::capability::{CLIPBOARD_TYPE, Descriptor, Manifest, Scope};
use lp_core::envelope::{Envelope, FactId};
use lp_core::facts::{
    DeviceFact, DeviceKind, GrantBasis, GrantFact, PersonFact, Registration, RevocationFact,
    RevocationKind, Role, RoleFact, SubordinateKey,
};
use lp_core::frame::{GossipFact, GossipHave, GossipWant, StreamHeader};
use lp_core::wire::{Bytes, Timestamp, TypeName};
use serde::Serialize;
use serde::de::DeserializeOwned;

fn schemas_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../schemas")
}

/// Maps `https://ligatureprotocol.org/schemas/<name>` to the local file.
struct LocalSchemas {
    by_id: HashMap<String, serde_json::Value>,
}

impl LocalSchemas {
    fn load() -> Self {
        let mut by_id = HashMap::new();
        for entry in std::fs::read_dir(schemas_dir()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                let v: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                let id = v["$id"].as_str().unwrap().to_owned();
                by_id.insert(id, v);
            }
        }
        Self { by_id }
    }

    fn validator(&self, name: &str) -> Validator {
        let schema =
            &self.by_id[&format!("https://ligatureprotocol.org/schemas/{name}.schema.json")];
        jsonschema::options()
            .with_retriever(Self::load())
            .build(schema)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
    }
}

impl Retrieve for LocalSchemas {
    fn retrieve(
        &self,
        uri: &Uri<String>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        self.by_id
            .get(uri.as_str())
            .cloned()
            .ok_or_else(|| format!("no local schema for {uri}").into())
    }
}

/// CBOR round trip through the typed message, then JSON validation.
fn check<T: Serialize + DeserializeOwned>(schemas: &LocalSchemas, schema: &str, value: &T) {
    let mut cbor = Vec::new();
    ciborium::into_writer(value, &mut cbor).unwrap();
    let back: T = ciborium::from_reader(cbor.as_slice()).unwrap();
    let json = serde_json::to_value(&back).unwrap();
    let validator = schemas.validator(schema);
    let errors: Vec<String> = validator
        .iter_errors(&json)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{schema}: {errors:?}\n{json}");
}

fn key(b: u8) -> PublicKey {
    PublicKey::from_bytes([b; 32])
}

fn at() -> Timestamp {
    Timestamp::from_unix(1_760_000_000).unwrap()
}

fn envelope() -> Envelope {
    Envelope {
        fact_type: TypeName::new(lp_core::facts::ROLE_TYPE),
        payload: Bytes(vec![0xa1, 0x61, 0x61, 0x01]),
        signer: key(1),
        signature: Bytes(vec![7; 64]),
    }
}

#[test]
fn stream_messages_validate_against_message_schema() {
    let s = LocalSchemas::load();
    check(&s, "message", &StreamHeader::new(StreamHeader::GOSSIP));
    check(&s, "message", &StreamHeader::new(CLIPBOARD_TYPE));
    check(
        &s,
        "message",
        &GossipHave::new(vec![FactId([1; 32]), FactId([2; 32])]),
    );
    check(&s, "message", &GossipWant::new(vec![]));
    check(&s, "message", &GossipFact::new(envelope()));
    let mut d = Descriptor::new(CLIPBOARD_TYPE, Scope::User);
    d.label = Some("laptop".to_owned());
    check(&s, "message", &Manifest::new(vec![d]));
    check(&s, "message", &Manifest::empty());
}

#[test]
fn facts_validate_against_their_schemas() {
    let s = LocalSchemas::load();
    check(&s, "envelope", &envelope());
    check(
        &s,
        "person",
        &PersonFact {
            master_key: key(1),
            display_name: "Alice".to_owned(),
            self_signing_key: SubordinateKey {
                key: key(2),
                master_signature: Bytes(vec![1; 64]),
            },
            user_signing_key: SubordinateKey {
                key: key(3),
                master_signature: Bytes(vec![2; 64]),
            },
            version: 1,
            updated_at: at(),
        },
    );
    check(
        &s,
        "role",
        &RoleFact {
            subject: key(4),
            role: Role::Binding,
            person: key(1),
            introducer: true,
            signed_at: at(),
        },
    );
    check(
        &s,
        "role",
        &RoleFact {
            subject: key(5),
            role: Role::Person,
            person: key(1),
            introducer: false,
            signed_at: at(),
        },
    );
    check(
        &s,
        "revocation",
        &RevocationFact {
            subject: key(4),
            kind: RevocationKind::Abandonment,
            person: key(1),
            revoked_at: at(),
        },
    );
    check(
        &s,
        "device",
        &DeviceFact {
            key: key(4),
            kind: DeviceKind::Binding,
            display_name: "laptop".to_owned(),
            platform: "linux".to_owned(),
            registration: None,
            person: Some(key(1)),
            principal: Some(key(6)),
            introducer: true,
            created_at: at(),
        },
    );
    check(
        &s,
        "device",
        &DeviceFact {
            key: key(6),
            kind: DeviceKind::Principal,
            display_name: "laptop".to_owned(),
            platform: "linux".to_owned(),
            registration: Some(Registration::Personal),
            person: None,
            principal: None,
            introducer: false,
            created_at: at(),
        },
    );
}

#[test]
fn grants_validate_against_the_grant_schema() {
    let s = LocalSchemas::load();
    check(
        &s,
        "grant",
        &GrantFact {
            grantor: key(4),
            grantee: key(7),
            grantee_person: None,
            capability: TypeName::new(CLIPBOARD_TYPE),
            basis: GrantBasis::Implicit,
            standing: true,
            expires_at: None,
            revoked_at: None,
            created_at: at(),
        },
    );
}

#[test]
fn a_wrong_message_fails_validation() {
    let s = LocalSchemas::load();
    let v = s.validator("message");
    assert!(
        !v.is_valid(&serde_json::json!({"type": "org.ligatureprotocol.stream"})),
        "header without purpose"
    );
    assert!(
        !v.is_valid(
            &serde_json::json!({"type": "org.ligatureprotocol.gossip.have", "facts": ["xyz"]})
        ),
        "bad fact id"
    );
    let d = s.validator("device");
    assert!(!d.is_valid(&serde_json::json!({
        "key": "nope", "kind": "binding", "displayName": "x", "platform": "linux",
        "createdAt": "2026-01-01T00:00:00Z"
    })));
}
