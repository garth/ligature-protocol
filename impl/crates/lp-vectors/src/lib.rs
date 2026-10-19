//! Conformance vectors: the JSON formats under `vectors/` and a loader.
//!
//! Test support only (milestone 1 decision LP-33). Other crates use this as a
//! dev-dependency; it is never part of a binary. The formats are documented in
//! `vectors/README.md`; a vector asserts behaviour on exact bytes (ADR 0035),
//! never that an implementation reproduces those bytes.
//!
//! Vectors are only ever written deliberately: a generating test runs when
//! `LP_WRITE_VECTORS=1` is set and refuses to overwrite a file that exists.

use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The format tag every vector file carries.
pub const FORMAT: &str = "ligature-vector/1";
/// Environment variable that lets a generating test write files.
pub const WRITE_ENV: &str = "LP_WRITE_VECTORS";

/// The `vectors/` directory at the repository root.
pub fn vectors_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../vectors")
}

/// Whether generating tests may write files in this run.
pub fn may_write() -> bool {
    std::env::var_os(WRITE_ENV).is_some_and(|v| v == "1")
}

/// Bytes as lowercase hex in JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hex(pub Vec<u8>);

impl Serialize for Hex {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        hex::decode(s).map(Hex).map_err(serde::de::Error::custom)
    }
}

/// What a vector expects of an implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum Expect {
    /// Everything verifies.
    Valid,
    /// Verification fails, for the stated reason.
    Invalid {
        /// Why, for a human; implementations need not match it.
        reason: String,
    },
}

/// One test key: `vectors/keys.json` holds a list of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestKey {
    /// Name other vectors refer to, such as `alice-master`.
    pub name: String,
    /// The 32-byte Ed25519 seed. Test only; never use outside vectors.
    pub seed: Hex,
    /// The public key as a multibase string (`z6Mk…`).
    pub public_key: String,
}

/// `vectors/keys.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyFile {
    /// Always [`FORMAT`].
    pub format: String,
    /// Marks the keys as test material.
    pub description: String,
    /// The keys.
    pub keys: Vec<TestKey>,
}

/// A signed-fact vector (`vectors/facts/*.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FactVector {
    /// Always [`FORMAT`].
    pub format: String,
    /// What the case shows.
    pub description: String,
    /// Names from `keys.json` the case uses.
    pub keys: Vec<String>,
    /// The fact as JSON, per its schema; the payload decodes to this.
    pub input: serde_json::Value,
    /// The envelope's type.
    #[serde(rename = "type")]
    pub fact_type: String,
    /// The payload bytes as transmitted.
    pub payload: Hex,
    /// The signer's public key, multibase.
    pub signer: String,
    /// The Ed25519 signature over `"ligature/fact/1" || type || payload`.
    pub signature: Hex,
    /// SHA-256 of the payload bytes.
    pub fact_id: Hex,
    /// Expected outcome.
    pub expect: Expect,
}

/// A ceremony-code vector (`vectors/ceremony/*.json`): exporter output to six digits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeVector {
    /// Always [`FORMAT`].
    pub format: String,
    /// What the case shows.
    pub description: String,
    /// The bytes the TLS exporter produced for the ceremony label.
    pub exporter: Hex,
    /// The six-digit code shown to the user.
    pub code: String,
}

/// Loads every `*.json` file in a directory as `T`, sorted by file name.
///
/// # Errors
///
/// Returns an error if the directory cannot be read or a file fails to parse,
/// naming the file.
pub fn load_dir<T: DeserializeOwned>(dir: &Path) -> Result<Vec<(PathBuf, T)>, Error> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| Error::Io(dir.to_path_buf(), e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| load_file(&p).map(|v| (p, v)))
        .collect()
}

/// Loads one vector file.
///
/// # Errors
///
/// Returns an error if the file cannot be read or parsed.
pub fn load_file<T: DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let text = fs::read_to_string(path).map_err(|e| Error::Io(path.to_path_buf(), e))?;
    serde_json::from_str(&text).map_err(|e| Error::Parse(path.to_path_buf(), e))
}

/// Writes a vector file, pretty-printed, refusing to overwrite.
///
/// # Errors
///
/// Returns an error if the file exists, or cannot be written.
pub fn write_file<T: Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    if path.exists() {
        return Err(Error::Exists(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::Io(parent.to_path_buf(), e))?;
    }
    let mut text =
        serde_json::to_string_pretty(value).map_err(|e| Error::Parse(path.to_path_buf(), e))?;
    text.push('\n');
    fs::write(path, text).map_err(|e| Error::Io(path.to_path_buf(), e))
}

/// Loader errors.
#[derive(Debug)]
pub enum Error {
    /// A file or directory could not be read or written.
    Io(PathBuf, std::io::Error),
    /// A file is not valid JSON for the expected type.
    Parse(PathBuf, serde_json::Error),
    /// A generating test tried to overwrite an existing vector.
    Exists(PathBuf),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Parse(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Exists(p) => write!(f, "{} exists; vectors are never overwritten", p.display()),
        }
    }
}

impl std::error::Error for Error {}
