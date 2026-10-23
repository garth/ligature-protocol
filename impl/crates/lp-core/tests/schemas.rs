//! Every message and fact the reference produces, round-tripped through CBOR
//! and validated as JSON against the schemas under `schemas/` (ADR 0020,
//! 0025, 0036). Cross-file references resolve to the local files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use jsonschema::{Retrieve, Uri, Validator};
use lp_core::PublicKey;
use lp_core::capability::{
    CLIPBOARD_TYPE, ClipboardGet, ClipboardPart, ClipboardSet, Descriptor, Manifest, Scope,
};
use lp_core::envelope::{Envelope, FactId};
use lp_core::facts::{
    DeviceFact, DeviceKind, GrantBasis, GrantFact, PersonFact, Registration, RevocationFact,
    RevocationKind, Role, RoleFact, SubordinateKey,
};
use lp_core::files::{
    FileDescriptor, FilesAccept, FilesBegin, FilesChunk, FilesDecline, FilesEnd, FilesFailed,
    FilesMessage, FilesOffer, FilesRequest, Reason, files_descriptor,
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

#[test]
fn clipboard_messages_validate_against_the_clipboard_schema() {
    let schemas = LocalSchemas::load();
    let hash = "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU".to_owned();
    // Text only, as milestone 2 sent it.
    check(
        &schemas,
        "capability.clipboard",
        &ClipboardSet::now("hello"),
    );
    // Text with a pushed HTML part and an announced PNG part.
    let mut png = FileDescriptor::new("p1", "clipboard.png", 3_000_000);
    png.mime = "image/png".to_owned();
    png.sha256 = Some(hash.clone());
    let mut rich = ClipboardSet::now("hello");
    rich.parts = vec![
        ClipboardPart::pushed("text/html", b"<b>hello</b>".to_vec()),
        ClipboardPart::announced("image/png", png.clone()),
    ];
    check(&schemas, "capability.clipboard", &rich);
    // A selection of files, always announced, with the paths as text.
    let mut files = ClipboardSet::now("/home/me/a.txt\n/home/me/b.txt");
    files.files = vec![
        FileDescriptor::new("f1", "a.txt", 10),
        FileDescriptor::new("f2", "b.txt", 20),
    ];
    check(&schemas, "capability.clipboard", &files);
    // Emptied, and a get.
    check(&schemas, "capability.clipboard", &ClipboardSet::empty());
    check(&schemas, "capability.clipboard", &ClipboardGet::default());
    // An announced part needs its hash, and a part needs bytes or a file.
    let validator = schemas.validator("capability.clipboard");
    let no_hash = serde_json::json!({
        "type": "org.ligatureprotocol.capability.clipboard.set",
        "setAt": "2026-10-22T00:00:00Z",
        "parts": [{"mime": "image/png", "file": {"id": "p1", "name": "x.png", "size": 1}}]
    });
    assert!(validator.iter_errors(&no_hash).next().is_some());
    let no_content = serde_json::json!({
        "type": "org.ligatureprotocol.capability.clipboard.set",
        "setAt": "2026-10-22T00:00:00Z",
        "parts": [{"mime": "image/png"}]
    });
    assert!(validator.iter_errors(&no_content).next().is_some());
}

#[test]
fn files_messages_validate_against_the_files_schema() {
    let schemas = LocalSchemas::load();
    let transfer = "AAAAAAAAAAAAAAAAAAAAAA".to_owned();
    let hash = "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU".to_owned();
    let mut file = FileDescriptor::new("f1", "photo.jpg", 12_345);
    file.mime = "image/jpeg".to_owned();
    file.sha256 = Some(hash.clone());
    file.modified = Some(at());
    let messages = [
        FilesMessage::Request(FilesRequest {
            transfer: transfer.clone(),
            ids: vec!["f1".to_owned()],
        }),
        FilesMessage::Offer(FilesOffer {
            transfer: transfer.clone(),
            files: vec![file, FileDescriptor::new("f2", "notes.txt", 0)],
        }),
        FilesMessage::Accept(FilesAccept {
            ids: vec!["f1".to_owned(), "f2".to_owned()],
        }),
        FilesMessage::Decline(FilesDecline {
            reason: Reason::Busy,
        }),
        FilesMessage::Begin(FilesBegin {
            transfer,
            id: "f1".to_owned(),
        }),
        FilesMessage::Chunk(FilesChunk {
            bytes: Bytes(vec![0, 1, 2, 255]),
        }),
        FilesMessage::End(FilesEnd { sha256: hash }),
        FilesMessage::Received,
        FilesMessage::Failed(FilesFailed {
            reason: Reason::SizeMismatch,
        }),
    ];
    for m in &messages {
        check(&schemas, "capability.files", m);
    }
    let manifest = Manifest::new(vec![files_descriptor(lp_core::files::FilesParams {
        send: true,
        receive: true,
        max_chunk_bytes: 262_144,
        max_streams: 4,
    })]);
    check(&schemas, "message", &manifest);
    // A chunk over the schema's bound of maxChunkBytes is not a schema matter,
    // but a transfer id of the wrong length is.
    let bad = serde_json::json!({
        "type": "org.ligatureprotocol.capability.files.begin",
        "transfer": "short",
        "id": "f1"
    });
    let validator = schemas.validator("capability.files");
    assert!(validator.iter_errors(&bad).next().is_some());
}
