//! The device's local store of signed facts. Spec chapter 06; LP-25.
//!
//! There is no shared database: this store *is* the truth as far as this
//! device knows. One SQLite database, `<data-dir>/store.sqlite`, holds every
//! signed fact byte-exact as it travels in gossip, keyed by its identifier
//! (SHA-256 of the payload bytes). Kind-specific views are added when a query
//! needs them. Private keys are never here (see [`crate::key_file`]).
//!
//! A fact whose signature fails or whose payload does not decode at load is
//! marked quarantined and logged; it is never deleted silently and never used,
//! and a copy that verifies, from a peer or the ceremony, heals the row.
//! Writes that belong together (the facts of an identity and the key file
//! that names it) go through one [`TrustStore::transaction`].

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use lp_core::envelope::{Envelope, FactId};
use lp_core::facts::{
    DEVICE_TYPE, GRANT_TYPE, LOCAL_ONLY_TYPES, PERSON_TYPE, REVOCATION_TYPE, ROLE_TYPE,
};
use lp_core::paths::Locations;
use lp_core::wire::{Bytes, Timestamp, TypeName};
use lp_core::{Error, PublicKey};
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::facts;

/// The schema version this daemon writes. Step 2 rewrote `created_at` in
/// the fixed-width form of [`Timestamp::to_rfc3339_fixed`], so the column
/// sorts as time.
pub const SCHEMA_VERSION: u32 = 3;

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
        if version < 2 {
            let tx = conn.unchecked_transaction().map_err(store_err)?;
            let rows: Vec<(String, String)> = {
                let mut read = tx
                    .prepare("SELECT hash, created_at FROM facts")
                    .map_err(store_err)?;
                let rows = read
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(store_err)?;
                rows.collect::<Result<_, _>>().map_err(store_err)?
            };
            for (hash, created_at) in rows {
                if let Ok(ts) = created_at.parse::<Timestamp>() {
                    tx.execute(
                        "UPDATE facts SET created_at = ?1 WHERE hash = ?2",
                        params![ts.to_rfc3339_fixed(), hash],
                    )
                    .map_err(store_err)?;
                }
            }
            tx.execute_batch("PRAGMA user_version = 2")
                .map_err(store_err)?;
            tx.commit().map_err(store_err)?;
        }
        if version < 3 {
            // The input layout (milestone 7, LP-142): which edge of this
            // device's screen leads to which device, a setting local to the
            // device and never gossiped.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS input_layout (
                    edge    TEXT PRIMARY KEY,
                    device  TEXT NOT NULL
                );
                PRAGMA user_version = 3;",
            )
            .map_err(store_err)?;
        }
        Ok(())
    }

    /// The input layout: each edge of this device's screen that leads to
    /// another device, with that device's key, in no particular order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn input_layout(&self) -> Result<Vec<(String, PublicKey)>, Error> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT edge, device FROM input_layout ORDER BY edge")
            .map_err(store_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(store_err)?;
        let mut out = Vec::new();
        for row in rows {
            let (edge, device) = row.map_err(store_err)?;
            match device.parse::<PublicKey>() {
                Ok(key) => out.push((edge, key)),
                Err(e) => {
                    tracing::warn!(edge, %e, "skipping an input layout row whose key does not read")
                }
            }
        }
        Ok(out)
    }

    /// Makes `edge` lead to `device`, replacing what it led to.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn set_input_edge(&self, edge: &str, device: &PublicKey) -> Result<(), Error> {
        self.conn()
            .execute(
                "INSERT INTO input_layout (edge, device) VALUES (?1, ?2)
                 ON CONFLICT(edge) DO UPDATE SET device = excluded.device",
                params![edge, device.to_string()],
            )
            .map(|_| ())
            .map_err(store_err)
    }

    /// Makes `edge` lead nowhere. Returns whether it led somewhere.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn unset_input_edge(&self, edge: &str) -> Result<bool, Error> {
        self.conn()
            .execute("DELETE FROM input_layout WHERE edge = ?1", params![edge])
            .map(|n| n > 0)
            .map_err(store_err)
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Stores an envelope after checking its signature and decoding it enough
    /// to know its subject and time. Storing a fact that is already present is
    /// a no-op, unless the stored copy is quarantined: a copy that verifies
    /// replaces it. Returns the fact's identifier.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidSignature`] or [`Error::Decode`] if the fact is
    /// bad, [`Error::Store`] on a database error.
    pub fn insert(&self, envelope: &Envelope) -> Result<FactId, Error> {
        self.transaction(|w| w.insert(envelope))
    }

    /// Runs `f` with a [`Writer`] whose inserts land together with whatever
    /// else `f` does: when `f` returns `Ok` the inserts are committed, and
    /// when it fails none of them take effect. This is how an identity's
    /// facts and the key file that names its person are written as one step:
    /// `f` saves the key file last, and a failure there leaves the store as
    /// it was. The store is locked while `f` runs, so `f` must not reach it
    /// through `self`.
    ///
    /// # Errors
    ///
    /// Returns what `f` returns, or [`Error::Store`] on a database error.
    pub fn transaction<T>(
        &self,
        f: impl FnOnce(&Writer<'_>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction().map_err(store_err)?;
        let out = f(&Writer { tx: &tx })?;
        tx.commit().map_err(store_err)?;
        Ok(out)
    }

    /// Whether a usable (not quarantined) copy of a fact is present.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn usable(&self, id: &FactId) -> Result<bool, Error> {
        self.conn()
            .query_row(
                "SELECT 1 FROM facts WHERE hash = ?1 AND quarantined = 0",
                [id.to_hex()],
                |_| Ok(()),
            )
            .optional()
            .map(|o| o.is_some())
            .map_err(store_err)
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
    /// except the local-only types ([`LOCAL_ONLY_TYPES`], grants today).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn ids(&self) -> Result<Vec<FactId>, Error> {
        let conn = self.conn();
        let placeholders = vec!["?"; LOCAL_ONLY_TYPES.len()].join(", ");
        let sql = format!(
            "SELECT hash FROM facts WHERE quarantined = 0 AND kind NOT IN ({placeholders}) ORDER BY hash"
        );
        let mut stmt = conn.prepare(&sql).map_err(store_err)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(LOCAL_ONLY_TYPES.iter()), |r| {
                r.get::<_, String>(0)
            })
            .map_err(store_err)?;
        rows.map(|r| {
            let hex = r.map_err(store_err)?;
            FactId::from_hex(&hex)
        })
        .collect()
    }

    /// Every usable revocation or abandonment naming `subject`, oldest first,
    /// whoever signed it. Whether one of them counts is decided against the
    /// person it names by [`crate::trust::is_revoked`] (LP-120); the store
    /// alone never says a key is revoked.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn revocations_naming(&self, subject: &PublicKey) -> Result<Vec<StoredFact>, Error> {
        self.query(
            "WHERE quarantined = 0 AND kind = ?1 AND subject = ?2 ORDER BY created_at, hash",
            &[&REVOCATION_TYPE, &subject.to_string()],
        )
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

    /// Quarantined facts, for diagnostics. A row quarantined because one of
    /// its columns no longer reads is logged at open and cannot be listed
    /// here; [`Self::quarantined_count`] still counts it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn quarantined(&self) -> Result<Vec<StoredFact>, Error> {
        self.query("WHERE quarantined = 1 ORDER BY hash", &[])
    }

    /// How many rows are quarantined, readable or not.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] on a database error.
    pub fn quarantined_count(&self) -> Result<i64, Error> {
        self.conn()
            .query_row(
                "SELECT COUNT(*) FROM facts WHERE quarantined = 1",
                [],
                |r| r.get(0),
            )
            .map_err(store_err)
    }

    /// Re-verifies every usable stored fact and quarantines those that fail,
    /// logging each loudly. Called on open. One pass over the table, a row at
    /// a time, and one transaction for the verdicts, so a large store costs
    /// one read and one commit; rows already quarantined are left alone. A
    /// row fails when its columns cannot be read, when its identifier or
    /// signature does not match its bytes, or when the subject or time it is
    /// indexed by disagrees with its payload.
    fn verify_all(&self) -> Result<(), Error> {
        let conn = self.conn();
        let mut bad: Vec<(i64, String, String)> = Vec::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT rowid, hash, kind, payload, signature, signer, subject, created_at
                     FROM facts WHERE quarantined = 0 ORDER BY rowid",
                )
                .map_err(store_err)?;
            let mut rows = stmt.query([]).map_err(store_err)?;
            while let Some(row) = rows.next().map_err(store_err)? {
                let rowid: i64 = row.get(0).map_err(store_err)?;
                if let Err(why) = verify_row(row) {
                    let hash = row.get::<_, String>(1).unwrap_or_default();
                    bad.push((rowid, hash, why));
                }
            }
        }
        if bad.is_empty() {
            return Ok(());
        }
        let tx = conn.unchecked_transaction().map_err(store_err)?;
        for (rowid, hash, why) in &bad {
            tracing::error!(fact = %hash, %why, "quarantining fact that fails verification");
            tx.execute("UPDATE facts SET quarantined = 1 WHERE rowid = ?1", [rowid])
                .map_err(store_err)?;
        }
        tx.commit().map_err(store_err)
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
        let mut out = Vec::new();
        for row in rows {
            let (hash, kind, payload, signature, signer, subject, created_at, quarantined) =
                row.map_err(store_err)?;
            let parsed = (|| -> Result<StoredFact, Error> {
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
            })();
            match parsed {
                Ok(fact) => out.push(fact),
                // Only reachable for a row already quarantined at open.
                Err(e) => {
                    tracing::warn!(fact = %hash, %e, "skipping a stored row whose columns do not read")
                }
            }
        }
        Ok(out)
    }
}

/// Inserts within one [`TrustStore::transaction`].
pub struct Writer<'a> {
    tx: &'a rusqlite::Transaction<'a>,
}

impl Writer<'_> {
    /// As [`TrustStore::insert`], within the transaction.
    ///
    /// # Errors
    ///
    /// As [`TrustStore::insert`].
    pub fn insert(&self, envelope: &Envelope) -> Result<FactId, Error> {
        facts::verify_signature(envelope)?;
        let (subject, created_at) = index_fields(envelope)?;
        let id = facts::id(envelope);
        self.tx
            .execute(
                "INSERT INTO facts (hash, kind, payload, signature, signer, subject, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (hash) DO UPDATE SET
                     kind = excluded.kind,
                     payload = excluded.payload,
                     signature = excluded.signature,
                     signer = excluded.signer,
                     subject = excluded.subject,
                     created_at = excluded.created_at,
                     quarantined = 0
                 WHERE facts.quarantined = 1",
                params![
                    id.to_hex(),
                    envelope.fact_type.as_str(),
                    envelope.payload.0,
                    envelope.signature.0,
                    envelope.signer.to_string(),
                    subject.to_string(),
                    created_at.to_rfc3339_fixed(),
                ],
            )
            .map_err(store_err)?;
        Ok(id)
    }
}

/// Checks one row of the facts table (`rowid, hash, kind, payload, signature,
/// signer, subject, created_at`) against its own bytes, naming the problem.
fn verify_row(row: &rusqlite::Row<'_>) -> Result<(), String> {
    let text = |i: usize| {
        row.get::<_, String>(i)
            .map_err(|e| format!("column {i} does not read as text: {e}"))
    };
    let blob = |i: usize| {
        row.get::<_, Vec<u8>>(i)
            .map_err(|e| format!("column {i} does not read as bytes: {e}"))
    };
    let id = FactId::from_hex(&text(1)?).map_err(|e| e.to_string())?;
    let envelope = Envelope {
        fact_type: TypeName(text(2)?),
        payload: Bytes(blob(3)?),
        signature: Bytes(blob(4)?),
        signer: text(5)?.parse().map_err(|e: Error| e.to_string())?,
    };
    let subject: PublicKey = text(6)?.parse().map_err(|e: Error| e.to_string())?;
    let created_at: Timestamp = text(7)?.parse().map_err(|e: Error| e.to_string())?;
    if facts::id(&envelope) != id {
        return Err("stored under the wrong identifier".to_owned());
    }
    facts::verify_signature(&envelope).map_err(|e| e.to_string())?;
    let (real_subject, real_created_at) = index_fields(&envelope).map_err(|e| e.to_string())?;
    if real_subject != subject {
        return Err("indexed under a subject the payload does not name".to_owned());
    }
    if real_created_at != created_at {
        return Err("indexed under a time the payload does not carry".to_owned());
    }
    Ok(())
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
    fn the_input_layout_is_rows_the_store_keeps() {
        let store = TrustStore::in_memory().unwrap();
        assert!(store.input_layout().unwrap().is_empty());
        let a = lp_crypto::signing::SigningKey::generate().unwrap();
        let b = lp_crypto::signing::SigningKey::generate().unwrap();
        store.set_input_edge("right", a.public()).unwrap();
        store.set_input_edge("left", b.public()).unwrap();
        store.set_input_edge("right", b.public()).unwrap();
        assert_eq!(
            store.input_layout().unwrap(),
            vec![
                ("left".to_owned(), *b.public()),
                ("right".to_owned(), *b.public())
            ]
        );
        assert!(store.unset_input_edge("left").unwrap());
        assert!(!store.unset_input_edge("left").unwrap());
        assert_eq!(store.input_layout().unwrap().len(), 1);
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
        store
            .insert(&facts::person_fact(&alice, "Alice", 1, at()).unwrap())
            .unwrap();
        let laptop = SigningKey::generate().unwrap();
        store
            .insert(
                &facts::role_fact(&alice, Role::Binding, *laptop.public(), false, at()).unwrap(),
            )
            .unwrap();
        let revoked = |store: &TrustStore| {
            crate::trust::is_revoked(store, alice.person(), laptop.public()).unwrap()
        };
        assert!(!revoked(&store));
        assert!(
            store
                .revocations_naming(laptop.public())
                .unwrap()
                .is_empty()
        );
        let later = Timestamp::from_unix(1_760_000_100).unwrap();
        let rev =
            facts::revocation_fact(&alice, *laptop.public(), RevocationKind::Revocation, later)
                .unwrap();
        store.insert(&rev).unwrap();
        assert!(revoked(&store));
        assert_eq!(store.revocations_naming(laptop.public()).unwrap().len(), 1);
        // A newer role fact over the same key does not restore it.
        let newer = Timestamp::from_unix(1_760_000_200).unwrap();
        store
            .insert(
                &facts::role_fact(&alice, Role::Binding, *laptop.public(), true, newer).unwrap(),
            )
            .unwrap();
        assert!(revoked(&store));
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

    #[test]
    fn a_valid_copy_heals_a_quarantined_row() {
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        let alice = PersonKeys::generate().unwrap();
        let person = facts::person_fact(&alice, "Alice", 1, at()).unwrap();
        let id = {
            let store = TrustStore::open(&loc).unwrap();
            store.insert(&person).unwrap()
        };
        let conn = Connection::open(loc.store()).unwrap();
        conn.execute(
            "UPDATE facts SET signature = X'00' WHERE hash = ?1",
            [id.to_hex()],
        )
        .unwrap();
        drop(conn);
        let store = TrustStore::open(&loc).unwrap();
        assert!(!store.usable(&id).unwrap());
        assert!(store.contains(&id).unwrap());
        assert_eq!(store.insert(&person).unwrap(), id, "the good copy is taken");
        assert!(store.usable(&id).unwrap());
        assert!(store.quarantined().unwrap().is_empty());
        assert_eq!(store.get(&id).unwrap().unwrap().envelope, person);
        // Healing is idempotent and never downgrades a good row.
        store.insert(&person).unwrap();
        assert_eq!(store.all().unwrap().len(), 1);
    }

    #[test]
    fn a_row_with_a_bad_column_is_quarantined_and_the_store_still_opens() {
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        let alice = PersonKeys::generate().unwrap();
        let bob = PersonKeys::generate().unwrap();
        let person = facts::person_fact(&alice, "Alice", 1, at()).unwrap();
        let laptop = SigningKey::generate().unwrap();
        let role = facts::role_fact(&alice, Role::Binding, *laptop.public(), true, at()).unwrap();
        let other = facts::person_fact(&bob, "Bob", 1, at()).unwrap();
        let (person_id, role_id, other_id) = {
            let store = TrustStore::open(&loc).unwrap();
            (
                store.insert(&person).unwrap(),
                store.insert(&role).unwrap(),
                store.insert(&other).unwrap(),
            )
        };
        let conn = Connection::open(loc.store()).unwrap();
        // One row indexed under the wrong subject, one whose time column is
        // not a time at all.
        conn.execute(
            "UPDATE facts SET subject = ?1 WHERE hash = ?2",
            params![bob.person().to_string(), role_id.to_hex()],
        )
        .unwrap();
        conn.execute(
            "UPDATE facts SET created_at = 'yesterday' WHERE hash = ?1",
            [other_id.to_hex()],
        )
        .unwrap();
        drop(conn);
        let store = TrustStore::open(&loc).unwrap();
        let usable: Vec<FactId> = store.all().unwrap().into_iter().map(|f| f.id).collect();
        assert_eq!(usable, vec![person_id]);
        assert_eq!(store.quarantined_count().unwrap(), 2);
        assert_eq!(
            store.quarantined().unwrap().len(),
            1,
            "the row whose time does not read cannot be listed"
        );
        assert!(
            store.about(bob.person()).unwrap().is_empty(),
            "not found by the lie"
        );
        // The valid copies heal both.
        store.insert(&role).unwrap();
        store.insert(&other).unwrap();
        assert_eq!(store.quarantined_count().unwrap(), 0);
        assert_eq!(store.about(laptop.public()).unwrap().len(), 1);
    }

    #[test]
    fn a_failed_step_rolls_the_whole_transaction_back() {
        let store = TrustStore::in_memory().unwrap();
        let alice = PersonKeys::generate().unwrap();
        let person = facts::person_fact(&alice, "Alice", 1, at()).unwrap();
        let laptop = SigningKey::generate().unwrap();
        let role = facts::role_fact(&alice, Role::Binding, *laptop.public(), true, at()).unwrap();
        let err = store
            .transaction(|w| {
                w.insert(&person)?;
                w.insert(&role)?;
                Err::<(), _>(Error::Io("the key file did not save".to_owned()))
            })
            .unwrap_err();
        assert!(matches!(err, Error::Io(_)));
        assert!(store.all().unwrap().is_empty(), "nothing landed");
        store
            .transaction(|w| {
                w.insert(&person)?;
                w.insert(&role)
            })
            .unwrap();
        assert_eq!(store.all().unwrap().len(), 2);
    }

    #[test]
    fn grants_are_not_in_the_gossip_list() {
        use lp_core::facts::{GrantBasis, GrantFact};
        let store = TrustStore::in_memory().unwrap();
        let alice = PersonKeys::generate().unwrap();
        store
            .insert(&facts::person_fact(&alice, "Alice", 1, at()).unwrap())
            .unwrap();
        let laptop = SigningKey::generate().unwrap();
        let phone = SigningKey::generate().unwrap();
        let grant = GrantFact {
            grantor: *laptop.public(),
            grantee: *phone.public(),
            grantee_person: None,
            capability: TypeName::new("org.ligatureprotocol.capability.clipboard"),
            basis: GrantBasis::Implicit,
            standing: true,
            expires_at: None,
            revoked_at: None,
            created_at: at(),
        };
        let id = store
            .insert(&facts::sign(&laptop, &grant).unwrap())
            .unwrap();
        assert_eq!(
            store.all().unwrap().len(),
            2,
            "kept for the device's own use"
        );
        assert!(
            !store.ids().unwrap().contains(&id),
            "never offered to a peer"
        );
    }

    #[test]
    fn a_store_written_at_version_1_is_brought_forward_and_sorts_by_time() {
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        let alice = PersonKeys::generate().unwrap();
        let whole: Timestamp = "2025-10-09T08:53:20Z".parse().unwrap();
        let half: Timestamp = "2025-10-09T08:53:20.5Z".parse().unwrap();
        let v1 = facts::person_fact(&alice, "Alice", 1, whole).unwrap();
        let v2 = facts::person_fact(&alice, "Alice", 2, half).unwrap();
        // The table exactly as the version 1 daemon created and filled it.
        create_private_dir(&loc.data_dir).unwrap();
        let conn = Connection::open(loc.store()).unwrap();
        conn.execute_batch(
            "CREATE TABLE facts (
                hash TEXT PRIMARY KEY, kind TEXT NOT NULL, payload BLOB NOT NULL,
                signature BLOB NOT NULL, signer TEXT NOT NULL, subject TEXT NOT NULL,
                created_at TEXT NOT NULL, quarantined INTEGER NOT NULL DEFAULT 0);
             CREATE INDEX facts_subject ON facts (subject);
             CREATE INDEX facts_kind ON facts (kind);
             PRAGMA user_version = 1;",
        )
        .unwrap();
        for (env, ts) in [(&v2, half), (&v1, whole)] {
            conn.execute(
                "INSERT INTO facts (hash, kind, payload, signature, signer, subject, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    facts::id(env).to_hex(),
                    env.fact_type.as_str(),
                    env.payload.0,
                    env.signature.0,
                    env.signer.to_string(),
                    alice.person().to_string(),
                    ts.to_rfc3339(),
                ],
            )
            .unwrap();
        }
        let old_order: Vec<String> = conn
            .prepare("SELECT created_at FROM facts ORDER BY created_at")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            old_order[0], "2025-10-09T08:53:20.5Z",
            "the old text misorders"
        );
        drop(conn);

        let store = TrustStore::open(&loc).unwrap();
        let version: u32 = store
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let all = store.all().unwrap();
        assert_eq!(all.len(), 2, "nothing was quarantined by the rewrite");
        assert_eq!(all[0].created_at, whole, "oldest first, as time");
        assert_eq!(all[1].created_at, half);
        assert_eq!(all[0].envelope, v1, "bytes untouched");
        assert!(store.quarantined().unwrap().is_empty());
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
