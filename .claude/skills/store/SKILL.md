---
name: store
description: USE when changing the SQLite trust store (its schema, migrations or queries) or where a daemon keeps state. Contains the store's conventions and the rules that keep every device's state local.
---

# The local store

Each device keeps one SQLite store (`store.sqlite` beside `keys.cbor` in the data
directory) through `rusqlite` with the bundled SQLite, behind `lp_identity::trust_store`.
Nothing else is state: manifests, promises, offers and notification copies live for the
session (ADR 0016), and what else touches disk is transient or the user's (the clipboard
cache and notification icons under the runtime directory, `Locations::clipboard_cache` and
`Locations::icons`, emptied on start; received files in the download folder, part files
until their hash checks). There is no server, no shared database and no sync of the store
itself; what crosses between devices is signed facts, gossiped.

## Schema and migrations

- The schema version is `PRAGMA user_version` (2 today), checked in `migrate()` at open: a
  store newer than the daemon is refused with an error, an older one is brought forward step
  by step (`if version < N { … PRAGMA user_version = N }`, each step in its own
  transaction). A change adds the next step and raises `SCHEMA_VERSION`; it never edits an
  earlier step, since stores at every earlier version exist on the owner's devices. Step 2
  rewrote `created_at` in the fixed-width form of `Timestamp::to_rfc3339_fixed` so the
  column sorts as time.
- `facts` is the table of record: `hash` (the fact id, SHA-256 of the payload bytes),
  `kind`, `payload` and `signature` as bytes exactly as received, `signer`, `subject`,
  `created_at`, `quarantined`. Payload bytes are never re-encoded on the way in or out
  (ADR 0035). Every row is verified at open (`verify_all`: one pass, one transaction for
  the verdicts, rows already quarantined skipped, the index columns checked against the
  payload) and every insert verifies its envelope; a row that fails is quarantined, never
  deleted, and a copy that verifies heals it (`Writer::insert` is an upsert over a
  quarantined row).
- Writes that belong together go through `TrustStore::transaction`: the inserts and a
  closing step (saving the key file) land as one, or not at all. `Daemon::init` and the
  ceremony's `persist` methods use it; a new write of several facts does too.
- Indexes are named `facts_<column>`; a new table follows `<table>_<column>`.
- Connection pragmas at open: WAL, `synchronous = FULL`, foreign keys on. The store is a
  `Mutex<Connection>`; every call is short and synchronous and runs on the thread that
  asks, the daemon's async tasks included, and no other code touches SQLite.
- Keys are not in the store: `keys.cbor` is a separate 0600 file (milestone 1), written to
  a temporary name, synced, renamed, and the directory synced.

## What goes in the store

Own keys' public parts and device records with role signatures, known persons, revocations
and abandonments, issued grants, group memberships, relays: the list in CLAUDE.md
"Architecture decisions" 5, and the layout settings a later milestone adds (the input
layout of milestone 7 is "a fact in the store", not a configuration file). A setting that
needs to reach the person's other devices becomes a signed fact and gossips; a setting local
to the device stays a row.

## What never goes in the store

A manifest or anything derived from one; clipboard content, notification bodies, message
bodies or files; a peer's address beyond the last-seen address the reconnect loop keeps in
memory; anything a helper (relay, discovery) says.

## Locations

`lp_core::paths::Locations` resolves the data and runtime directories from the command line,
the environment and the platform defaults; both binaries and the Android node go through it.
A change to where state lives is a change there and in the units, the install script, the
README and chapter 08, as LP-85 did. An explicit data directory with no runtime directory
given keeps the socket, the clipboard cache and the icons beside the store, so two daemons
on one machine never share one.

## Tests

The store's tests are in `trust_store.rs` with `tempfile` directories: a reopen byte-exact,
the too-new-schema refusal, a corrupted row quarantined and healed by a good copy, a row
with a lying subject column or an unreadable time column, a transaction rolled back by a
failing step, grants absent from the gossip list, file modes, and a store written at
version 1 brought forward. A migration step gets such a test: create the previous version's
table by hand, fill it, open, and read it back. The `persistence` test in
`ligatured/tests` restarts a daemon on its store; `gossip.rs` there covers what a peer may
put in it.
