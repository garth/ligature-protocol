//! The private keys on disk: `<data-dir>/keys.cbor`, mode 0600 (LP-25).
//!
//! Private keys never enter the database, so the store can be inspected or
//! copied without touching secrets. Wrapping this file with a platform
//! keystore is guidance under chapter 08, not a milestone 1 task.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use lp_core::Error;
use lp_core::wire::Bytes;
use lp_crypto::signing::SigningKey;
use serde::{Deserialize, Serialize};

use crate::cross_signing::PersonKeys;

/// Everything secret this daemon holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyFile {
    /// Format tag; always 1.
    pub version: u32,
    /// The person's three keys, if this device holds them (the first device
    /// does; a newcomer receives the self-signing key only if it is an
    /// introducer, which milestone 1 does not persist separately).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<PersonSeeds>,
    /// This account's binding key on this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<Bytes>,
    /// This device's principal key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<Bytes>,
}

/// Seeds of a person's keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonSeeds {
    /// Master key seed.
    pub master: Bytes,
    /// Self-signing key seed.
    pub self_signing: Bytes,
    /// User-signing key seed.
    pub user_signing: Bytes,
}

impl KeyFile {
    /// The current format version.
    pub const VERSION: u32 = 1;

    /// Reads the file, or returns an empty key file if it does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] or [`Error::Decode`].
    pub fn load(path: &Path) -> Result<Self, Error> {
        match fs::read(path) {
            Ok(bytes) => {
                let file: Self = ciborium::from_reader(bytes.as_slice())
                    .map_err(|e| Error::Decode(format!("{}: {e}", path.display())))?;
                if file.version > Self::VERSION {
                    return Err(Error::Decode(format!(
                        "{}: key file version {} is newer than this daemon",
                        path.display(),
                        file.version
                    )));
                }
                Ok(file)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                version: Self::VERSION,
                ..Self::default()
            }),
            Err(e) => Err(Error::Io(format!("{}: {e}", path.display()))),
        }
    }

    /// Writes the file atomically with mode 0600.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] or [`Error::Decode`] (encoding).
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(self, &mut bytes)
            .map_err(|e| Error::Decode(format!("encode: {e}")))?;
        let tmp = path.with_extension("cbor.tmp");
        let io = |e: std::io::Error| Error::Io(format!("{}: {e}", path.display()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp).map_err(io)?;
        f.write_all(&bytes).map_err(io)?;
        f.sync_all().map_err(io)?;
        drop(f);
        fs::rename(&tmp, path).map_err(io)
    }

    /// The person's keys, rebuilt from their seeds.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if a seed is not 32 bytes.
    pub fn person_keys(&self) -> Result<Option<PersonKeys>, Error> {
        let Some(p) = &self.person else {
            return Ok(None);
        };
        Ok(Some(PersonKeys::from_keys(
            key(&p.master)?,
            key(&p.self_signing)?,
            key(&p.user_signing)?,
        )))
    }

    /// Records the person's keys.
    pub fn set_person(&mut self, keys: &PersonKeys) {
        self.person = Some(PersonSeeds {
            master: seed(&keys.master),
            self_signing: seed(&keys.self_signing),
            user_signing: seed(&keys.user_signing),
        });
    }

    /// The binding key, if any.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if the seed is not 32 bytes.
    pub fn binding_key(&self) -> Result<Option<SigningKey>, Error> {
        self.binding.as_ref().map(key).transpose()
    }

    /// The principal key, if any.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if the seed is not 32 bytes.
    pub fn principal_key(&self) -> Result<Option<SigningKey>, Error> {
        self.principal.as_ref().map(key).transpose()
    }

    /// Records the device keys.
    pub fn set_device_keys(&mut self, binding: &SigningKey, principal: &SigningKey) {
        self.binding = Some(seed(binding));
        self.principal = Some(seed(principal));
    }
}

fn seed(k: &SigningKey) -> Bytes {
    Bytes(k.seed().to_vec())
}

fn key(b: &Bytes) -> Result<SigningKey, Error> {
    let seed: [u8; 32] =
        b.0.as_slice()
            .try_into()
            .map_err(|_| Error::Decode("key seed must be 32 bytes".to_owned()))?;
    Ok(SigningKey::from_seed(seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.cbor");
        let empty = KeyFile::load(&path).unwrap();
        assert!(empty.person.is_none());
        let mut file = empty;
        let person = PersonKeys::generate().unwrap();
        let (b, p) = (
            SigningKey::generate().unwrap(),
            SigningKey::generate().unwrap(),
        );
        file.set_person(&person);
        file.set_device_keys(&b, &p);
        file.save(&path).unwrap();
        let back = KeyFile::load(&path).unwrap();
        assert_eq!(
            back.person_keys().unwrap().unwrap().person(),
            person.person()
        );
        assert_eq!(back.binding_key().unwrap().unwrap().public(), b.public());
        assert_eq!(back.principal_key().unwrap().unwrap().public(), p.public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
