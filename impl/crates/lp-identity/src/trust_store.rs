//! The device's local store of signed facts. Spec chapter 06; LP-25.
//!
//! There is no shared database: this store *is* the truth as far as this
//! device knows. One SQLite database, `<data-dir>/store.sqlite`, holds every
//! signed fact byte-exact as it travels in gossip, keyed by its identifier
//! (SHA-256 of the payload bytes). Kind-specific views are added when a query
//! needs them. Private keys are never here (see [`crate::key_file`]).
//!
//! A fact whose signature fails or whose payload does not decode at load is
//! marked quarantined and logged; it is never deleted silently and never used.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use lp_core::envelope::{Envelope, FactId};
use lp_core::facts::{DEVICE_TYPE, GRANT_TYPE, PERSON_TYPE, REVOCATION_TYPE, ROLE_TYPE};
use lp_core::paths::Locations;
use lp_core::wire::{Bytes, Timestamp, TypeName};
use lp_core::{Error, PublicKey};
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::facts;

/// The schema version this daemon writes.
pub const SCHEMA_VERSION: u32 = 1;

/// The persistent local store of signed facts.
pub struct TrustStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for TrustStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TrustStore")
    }
}

/// A stored fact: the envelope plus what the store indexes it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFact {
    /// The identifier.
    pub id: FactId,
    /// The envelope, byte-exact.
    pub envelope: Envelope,
    /// The key the fact is about, for lookups.
    pub subject: PublicKey,
    /// When the fact says it was made.
    pub created_at: Timestamp,
    /// Whether the fact failed verification at load.
    pub quarantined: bool,
}

impl TrustStore {
    /// Opens or creates the store at the data directory of `locations`,
    /// runs migrations, then verifies every stored fact and quarantines
    /// those that fail.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] for directory or file problems, [`Error::Store`]
    /// for SQLite errors, and [`Error::Store`] if the database is newer than
    /// this daemon.
    pub fn open(locations: &Locations) -> Result<Self, Error> {
        create_private_dir(&locations.data_dir)?;
        let path = locations.store();
        let fresh = !path.exists();
        let conn = Connection::open(&path).map_err(store_err)?;
        if fresh {
            set_private(&path)?;
        }
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;",
        )
        .map_err(store_err)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        for suffix in ["-wal", "-shm"] {
            let side = side_file(&path, suffix);
            if side.exists() {
                set_private(&side)?;
            }
        }
        store.verify_all()?;
        Ok(store)
    }

    /// Opens an in-memory store, for tests and tools.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] for SQLite errors.
    pub fn in_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory().map_err(store_err)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), Error> {
        let conn = self.conn();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(store_err)?;
        if version > SCHEMA_VERSION {
            return Err(Error::Store(format!(
                "store schema version {version} is newer than this daemon ({SCHEMA_VERSION})"
            )));
        }
        if version < 1 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS facts (
                    hash        TEXT PRIMARY KEY,
                    kind        TEXT NOT NULL,
                    payload     BLOB NOT NULL,
                    signature   BLOB NOT NULL,
                    signer      TEXT NOT NULL,
                    subject     TEXT NOT NULL,
                    created_at  TEXT NOT NULL,
                    quarantined INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS facts_subject ON facts (subject);
                CREATE INDEX IF NOT EXISTS facts_kind ON facts (kind);
                PRAGMA user_version = 1;",
            )
            .map_err(store_err)?;
        }
        Ok(())
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Stores an envelope after checking its signature and decoding it enough
    /// to know its subject and time. Storing a fact that is already present is
    /// a no-op. Returns the fact's identifier.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidSignature`] or [`Error::Decode`] if the fact is
    /// bad, [`Error::Store`] on a database error.
    pub fn insert(&self, envelope: &Envelope) -> Result<FactId, Error> {
        facts::verify_signature(envelope)?;
        let (subject, created_at) = index_fields(envelope)?;
        let id = facts::id(envelope);
        self.conn()
            .execute(
                "INSERT OR IGNORE INTO facts (hash, kind, payload, signature, signer, subject, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id.to_hex(),
                    envelope.fact_type.as_str(),
                    envelope.payload.0,
                    envelope.signature.0,
                    envelope.signer.to_string(),
                    subject.to_string(),
                    created_at.to_rfc3339(),
                ],
            )
            .map_err(store_err)?;
        Ok(id)
    }

    /// Whether a fact is present (quarantined or not).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn contains(&self, id: &FactId) -> Result<bool, Error> {
        self.conn()
            .query_row("SELECT 1 FROM facts WHERE hash = ?1", [id.to_hex()], |_| {
                Ok(())
            })
            .optional()
            .map(|o| o.is_some())
            .map_err(store_err)
    }

    /// One fact by identifier, quarantined or not.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn get(&self, id: &FactId) -> Result<Option<StoredFact>, Error> {
        let mut out = self.query("WHERE hash = ?1", &[&id.to_hex()])?;
        Ok(out.pop())
    }

    /// Every usable (not quarantined) fact, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn all(&self) -> Result<Vec<StoredFact>, Error> {
        self.query("WHERE quarantined = 0 ORDER BY created_at, hash", &[])
    }

    /// Usable facts of one type, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn of_type(&self, fact_type: &str) -> Result<Vec<StoredFact>, Error> {
        self.query(
            "WHERE quarantined = 0 AND kind = ?1 ORDER BY created_at, hash",
            &[&fact_type],
        )
    }

    /// Usable facts about one key, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn about(&self, subject: &PublicKey) -> Result<Vec<StoredFact>, Error> {
        self.query(
            "WHERE quarantined = 0 AND subject = ?1 ORDER BY created_at, hash",
            &[&subject.to_string()],
        )
    }

    /// Identifiers of every usable fact that may be gossiped: everything
    /// except the local-only types (grants).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn ids(&self) -> Result<Vec<FactId>, Error> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT hash FROM facts WHERE quarantined = 0 AND kind != ?1 ORDER BY hash")
            .map_err(store_err)?;
        let rows = stmt
            .query_map([GRANT_TYPE], |r| r.get::<_, String>(0))
            .map_err(store_err)?;
        rows.map(|r| {
            let hex = r.map_err(store_err)?;
            FactId::from_hex(&hex)
        })
        .collect()
    }

    /// Whether any usable revocation or abandonment names `subject`. A revoked
    /// key stays revoked: there is no fact that undoes a revocation, and a
    /// later role fact over the same key does not restore it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn is_revoked(&self, subject: &PublicKey) -> Result<bool, Error> {
        self.conn()
            .query_row(
                "SELECT 1 FROM facts WHERE quarantined = 0 AND kind = ?1 AND subject = ?2 LIMIT 1",
                params![REVOCATION_TYPE, subject.to_string()],
                |_| Ok(()),
            )
            .optional()
            .map(|o| o.is_some())
            .map_err(store_err)
    }

    /// Every usable revocation and abandonment, oldest first. Gossip sends
    /// these before anything else.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn revocations(&self) -> Result<Vec<StoredFact>, Error> {
        self.of_type(REVOCATION_TYPE)
    }

    /// Quarantined facts, for diagnostics.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn quarantined(&self) -> Result<Vec<StoredFact>, Error> {
        self.query("WHERE quarantined = 1 ORDER BY hash", &[])
    }

    /// Re-verifies every stored fact and quarantines those that fail,
    /// logging each loudly. Called on open.
    fn verify_all(&self) -> Result<(), Error> {
        let everything = self.query("ORDER BY hash", &[])?;
        for fact in everything {
            let actual_id = facts::id(&fact.envelope);
            let problem = if actual_id != fact.id {
                Some("stored under the wrong identifier".to_owned())
            } else if let Err(e) = facts::verify_signature(&fact.envelope) {
                Some(e.to_string())
            } else if let Err(e) = index_fields(&fact.envelope) {
                Some(e.to_string())
            } else {
                None
            };
            if let Some(why) = problem {
                if !fact.quarantined {
                    tracing::error!(fact = %fact.id, kind = %fact.envelope.fact_type, %why, "quarantining fact that fails verification");
                }
                self.conn()
                    .execute(
                        "UPDATE facts SET quarantined = 1 WHERE hash = ?1",
                        [fact.id.to_hex()],
                    )
                    .map_err(store_err)?;
            }
        }
        Ok(())
    }

    fn query(&self, clause: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<StoredFact>, Error> {
        let conn = self.conn();
        let sql = format!(
            "SELECT hash, kind, payload, signature, signer, subject, created_at, quarantined FROM facts {clause}"
        );
        let mut stmt = conn.prepare(&sql).map_err(store_err)?;
        let rows = stmt
            .query_map(args, |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, bool>(7)?,
                ))
            })
            .map_err(store_err)?;
        rows.map(|row| {
            let (hash, kind, payload, signature, signer, subject, created_at, quarantined) =
                row.map_err(store_err)?;
            Ok(StoredFact {
                id: FactId::from_hex(&hash)?,
                envelope: Envelope {
                    fact_type: TypeName(kind),
                    payload: Bytes(payload),
                    signer: signer.parse()?,
                    signature: Bytes(signature),
                },
                subject: subject.parse()?,
                created_at: created_at.parse()?,
                quarantined,
            })
        })
        .collect()
    }
}

/// The subject key and timestamp a fact is indexed by, from its decoded payload.
fn index_fields(envelope: &Envelope) -> Result<(PublicKey, Timestamp), Error> {
    use lp_core::facts::{DeviceFact, GrantFact, PersonFact, RevocationFact, RoleFact};
    match envelope.fact_type.as_str() {
        PERSON_TYPE => {
            let f: PersonFact = facts::decode(envelope)?;
            Ok((f.master_key, f.updated_at))
        }
        ROLE_TYPE => {
            let f: RoleFact = facts::decode(envelope)?;
            Ok((f.subject, f.signed_at))
        }
        REVOCATION_TYPE => {
            let f: RevocationFact = facts::decode(envelope)?;
            Ok((f.subject, f.revoked_at))
        }
        DEVICE_TYPE => {
            let f: DeviceFact = facts::decode(envelope)?;
            Ok((f.key, f.created_at))
        }
        GRANT_TYPE => {
            let f: GrantFact = facts::decode(envelope)?;
            Ok((f.grantee, f.created_at))
        }
        other => Err(Error::Decode(format!("unknown fact type {other}"))),
    }
}

fn store_err(e: rusqlite::Error) -> Error {
    Error::Store(e.to_string())
}

fn side_file(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

fn create_private_dir(dir: &Path) -> Result<(), Error> {
    if !dir.exists() {
        fs::create_dir_all(dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
    }
    Ok(())
}

fn set_private(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cross_signing::PersonKeys;
    use lp_core::facts::Role;
    use lp_crypto::signing::SigningKey;

    fn at() -> Timestamp {
        Timestamp::from_unix(1_760_000_000).unwrap()
    }

    fn locations(dir: &Path) -> Locations {
        Locations::resolve(Some(dir), None)
    }

    #[test]
    fn facts_survive_a_reopen_byte_exact() {
        let dir = tempfile::tempdir().unwrap();
        let alice = PersonKeys::generate().unwrap();
        let person = facts::person_fact(&alice, "Alice", 1, at()).unwrap();
        let laptop = SigningKey::generate().unwrap();
        let role = facts::role_fact(&alice, Role::Binding, *laptop.public(), true, at()).unwrap();
        let id = {
            let store = TrustStore::open(&locations(dir.path())).unwrap();
            store.insert(&person).unwrap();
            let id = store.insert(&role).unwrap();
            assert_eq!(store.insert(&role).unwrap(), id, "idempotent");
            id
        };
        let store = TrustStore::open(&locations(dir.path())).unwrap();
        let all = store.all().unwrap();
        assert_eq!(all.len(), 2);
        let got = store.get(&id).unwrap().unwrap();
        assert_eq!(got.envelope, role);
        assert_eq!(got.subject, *laptop.public());
        assert_eq!(store.about(laptop.public()).unwrap().len(), 1);
        assert_eq!(store.of_type(PERSON_TYPE).unwrap().len(), 1);
        assert_eq!(store.ids().unwrap().len(), 2);
        assert!(store.contains(&id).unwrap());
        assert!(store.quarantined().unwrap().is_empty());
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        drop(TrustStore::open(&loc).unwrap());
        let conn = Connection::open(loc.store()).unwrap();
        conn.execute_batch("PRAGMA user_version = 99").unwrap();
        drop(conn);
        assert!(matches!(TrustStore::open(&loc), Err(Error::Store(_))));
    }

    #[test]
    fn a_corrupted_fact_is_quarantined_and_the_store_still_opens() {
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        let alice = PersonKeys::generate().unwrap();
        let person = facts::person_fact(&alice, "Alice", 1, at()).unwrap();
        let id = {
            let store = TrustStore::open(&loc).unwrap();
            store.insert(&person).unwrap()
        };
        let conn = Connection::open(loc.store()).unwrap();
        let mut payload = person.payload.0.clone();
        payload[5] ^= 0xff;
        conn.execute(
            "UPDATE facts SET payload = ?1 WHERE hash = ?2",
            params![payload, id.to_hex()],
        )
        .unwrap();
        drop(conn);
        let store = TrustStore::open(&loc).unwrap();
        assert!(store.all().unwrap().is_empty(), "not used");
        assert_eq!(store.quarantined().unwrap().len(), 1, "not deleted");
        assert!(store.contains(&id).unwrap());
    }

    #[test]
    fn a_revocation_is_kept_and_cannot_be_undone() {
        use lp_core::facts::RevocationKind;
        let store = TrustStore::in_memory().unwrap();
        let alice = PersonKeys::generate().unwrap();
        let laptop = SigningKey::generate().unwrap();
        store
            .insert(
                &facts::role_fact(&alice, Role::Binding, *laptop.public(), false, at()).unwrap(),
            )
            .unwrap();
        assert!(!store.is_revoked(laptop.public()).unwrap());
        let later = Timestamp::from_unix(1_760_000_100).unwrap();
        let rev =
            facts::revocation_fact(&alice, *laptop.public(), RevocationKind::Revocation, later)
                .unwrap();
        store.insert(&rev).unwrap();
        assert!(store.is_revoked(laptop.public()).unwrap());
        // A newer role fact over the same key does not restore it.
        let newer = Timestamp::from_unix(1_760_000_200).unwrap();
        store
            .insert(
                &facts::role_fact(&alice, Role::Binding, *laptop.public(), true, newer).unwrap(),
            )
            .unwrap();
        assert!(store.is_revoked(laptop.public()).unwrap());
        assert_eq!(store.revocations().unwrap().len(), 1);
        assert_eq!(
            store.about(laptop.public()).unwrap().len(),
            3,
            "everything is retained for gossip"
        );
    }

    #[test]
    fn bad_envelopes_are_refused_on_insert() {
        let store = TrustStore::in_memory().unwrap();
        let alice = PersonKeys::generate().unwrap();
        let mut person = facts::person_fact(&alice, "Alice", 1, at()).unwrap();
        person.payload.0[3] ^= 1;
        assert!(matches!(
            store.insert(&person),
            Err(Error::InvalidSignature(_))
        ));
        let unknown = facts::sign_as(
            &alice.self_signing,
            &TypeName::new("org.example.thing"),
            &1u8,
        )
        .unwrap();
        assert!(matches!(store.insert(&unknown), Err(Error::Decode(_))));
        assert!(store.all().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn files_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let loc = locations(&data);
        let store = TrustStore::open(&loc).unwrap();
        let alice = PersonKeys::generate().unwrap();
        store
            .insert(&facts::person_fact(&alice, "Alice", 1, at()).unwrap())
            .unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&data), 0o700);
        assert_eq!(mode(&loc.store()), 0o600);
        let wal = side_file(&loc.store(), "-wal");
        if wal.exists() {
            assert_eq!(mode(&wal), 0o600);
        }
    }
}
