---
name: store
description: USE when changing the SQLite trust store (its schema, migrations or queries) or where a daemon keeps state. Contains the store's conventions and the rules that keep every device's state local.
---

# The local store

Each device keeps one SQLite store (`store.sqlite` beside `keys.cbor` in the data
directory) through `rusqlite` with the bundled SQLite, behind `lp_identity::trust_store`.
Nothing else persists: manifests, promises, offers, icons and notification copies live for
the session (ADR 0016). There is no server, no shared database and no sync of the store
itself; what crosses between devices is signed facts, gossiped.

## Schema and migrations

- The schema version is `PRAGMA user_version`, checked in `migrate()` at open: a store newer
  than the daemon is refused with an error, an older one is brought forward step by step
  (`if version < N { … PRAGMA user_version = N }`). A change adds the next step and raises
  `SCHEMA_VERSION`; it never edits an earlier step, since stores at every earlier version
  exist on the owner's devices.
- `facts` is the table of record: `hash` (the fact id, SHA-256 of the payload bytes),
  `kind`, `payload` and `signature` as bytes exactly as received, `signer`, `subject`,
  `created_at`, `quarantined`. Payload bytes are never re-encoded on the way in or out
  (ADR 0035); a row is verified when read and quarantined, not deleted, when it fails.
- Indexes are named `facts_<column>`; a new table follows `<table>_<column>`.
- Connection pragmas at open: WAL, `synchronous = FULL`, foreign keys on. The store is a
  `Mutex<Connection>` used from a dedicated path (`spawn_blocking` where the caller is
  async); no other code touches SQLite.
- Keys are not in the store: `keys.cbor` is a separate 0600 file (milestone 1).

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
README and chapter 08, as LP-85 did. An explicit data directory keeps the socket beside the
store so two daemons on one machine never share one.

## Tests

The store's tests are in `trust_store.rs` with `tempfile` directories, including the
too-new-schema refusal and quarantine; a migration step gets a test that opens a store
written at the previous version and reads it back after the step. The `persistence` test
in `ligatured/tests` restarts a daemon on its store.
