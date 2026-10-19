//! Conformance: the test keys in `vectors/keys.json` derive their public keys.

use lp_crypto::signing::SigningKey;
use lp_vectors::{FORMAT, KeyFile, load_file, vectors_dir};

#[test]
fn every_test_seed_derives_its_public_key() {
    let file: KeyFile = load_file(&vectors_dir().join("keys.json")).expect("keys.json");
    assert_eq!(file.format, FORMAT);
    assert!(!file.keys.is_empty());
    for key in &file.keys {
        let seed: [u8; 32] = key.seed.0.clone().try_into().expect("32-byte seed");
        let derived = SigningKey::from_seed(seed);
        assert_eq!(
            derived.public().to_multibase(),
            key.public_key,
            "{}",
            key.name
        );
    }
}
