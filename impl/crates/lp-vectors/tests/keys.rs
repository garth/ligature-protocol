//! The test keys: generated once on demand, checked on every run.

use ed25519_dalek::SigningKey;
use lp_vectors::{FORMAT, Hex, KeyFile, TestKey, load_file, may_write, vectors_dir, write_file};

const NAMES: [&str; 6] = [
    "alice-master",
    "alice-self",
    "alice-user",
    "laptop-binding",
    "laptop-principal",
    "mallory",
];

/// The `ed25519-pub` multicodec prefix (0xed as an unsigned varint).
const ED25519_PUB: [u8; 2] = [0xed, 0x01];

fn render(public: &[u8; 32]) -> String {
    let mut bytes = ED25519_PUB.to_vec();
    bytes.extend_from_slice(public);
    multibase::encode(multibase::Base::Base58Btc, bytes)
}

#[test]
fn write_keys_on_demand() {
    let path = vectors_dir().join("keys.json");
    if !may_write() || path.exists() {
        return;
    }
    let keys = NAMES
        .iter()
        .map(|name| {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).expect("operating system randomness");
            let key = SigningKey::from_bytes(&seed);
            TestKey {
                name: (*name).to_owned(),
                seed: Hex(key.to_bytes().to_vec()),
                public_key: render(&key.verifying_key().to_bytes()),
            }
        })
        .collect();
    let file = KeyFile {
        format: FORMAT.to_owned(),
        description: "Test keys for the conformance vectors. Never use them for anything else."
            .to_owned(),
        keys,
    };
    write_file(&path, &file).expect("write keys.json");
}

#[test]
fn every_seed_derives_its_public_key() {
    let path = vectors_dir().join("keys.json");
    if may_write() && !path.exists() {
        // A generation run; the writer test may not have finished yet.
        return;
    }
    let file: KeyFile = load_file(&path).expect("keys.json");
    assert_eq!(file.format, FORMAT);
    assert_eq!(file.keys.len(), NAMES.len());
    for (key, name) in file.keys.iter().zip(NAMES) {
        assert_eq!(key.name, name);
        let seed: [u8; 32] = key.seed.0.clone().try_into().expect("32-byte seed");
        let derived = render(&SigningKey::from_bytes(&seed).verifying_key().to_bytes());
        assert_eq!(derived, key.public_key, "{name}");
        assert!(key.public_key.starts_with("z6Mk"), "{name}");
    }
}
