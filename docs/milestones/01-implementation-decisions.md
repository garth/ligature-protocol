# Milestone 1: implementation decisions

Choices that are not part of the protocol but that the reference implementation needs
settled before the code that depends on them. Each entry names its ticket. Where a choice
turned out to touch the protocol it became an ADR instead and is only referenced here.

## LP-25: local store is SQLite

**Decision.** Each device's local store (chapter 06) is one SQLite database,
`<data-dir>/store.sqlite`, through `rusqlite` with the `bundled` feature so both platforms
compile the same SQLite and need no system library. Chosen over a directory of CBOR files
for the capabilities it gives later: queries across facts and grants, and durable queues,
for example outbound gossip or pending prompts.

**Keys are not in the database.** Private keys live in `<data-dir>/keys.cbor`, mode 0600,
so the database can be inspected, copied or exported without touching secrets. Wrapping
that file with a platform keystore is guidance under chapter 08, not a milestone 1 task.

**Schema.** One `facts` table holds every signed fact as it travels in gossip: `hash`
(primary key: SHA-256 of the payload bytes as received, lowercase hex, which is also the
fact's gossip identifier), `kind` (the envelope's type), `payload` and `signature` (the
bytes, never re-encoded), `signer`, `subject`, `created_at`, and `quarantined`. Kind-
specific columns and views are added when a query needs them, not in advance. Grants,
group documents and relays are facts like any other. Schema versioning uses `PRAGMA
user_version`; migrations are code in the daemon run once at startup; a database with a
higher version than the daemon knows is refused.

**Durability.** WAL journal mode, `synchronous=FULL` (writes are rare), foreign keys on.
Directory 0700, database and WAL files 0600, set at creation.

**Bad data.** A fact whose CBOR fails to decode or whose signature fails to verify at load
is marked `quarantined` and logged loudly; it is never deleted silently and never used. The
daemon starts without it.

**Access from async code.** `rusqlite` is synchronous; the daemon owns one connection on a
dedicated thread (or uses `spawn_blocking`) behind the `TrustStore` type, so the rest of
the code never sees SQLite.

**Locations.** `$XDG_DATA_HOME/ligature/` (default `~/.local/share/ligature/`) on Linux and
`~/Library/Application Support/ligature/` on macOS, resolved by one function and overridable
with `--data-dir` (LP-32). The control socket lives in the runtime directory, not here
(LP-30).

## LP-26: the key is the identifier

Became [ADR 0034](../adr/0034-keys-are-identifiers.md) because it changes every schema's
references. There is no `deviceId`; devices and people are referenced by their public keys.
Display names default to the host name and are user-overridable. Implementation note:
`lp-core::DeviceKey` replaces `DeviceId`; terminals and logs show the display name and, when
needed, a short fingerprint of the key, which is never stored or compared.

## LP-27: crates for crypto, encoding, runtime and store

| Need | Crate | Why |
| --- | --- | --- |
| Ed25519 | `ed25519-dalek` 3 | The standard implementation; shares `curve25519-dalek` with the rustls stack; MSRV 1.85 |
| Randomness | `rand_core` 0.9 with `os_rng` | Key generation source for dalek |
| Hashing | `sha2` 0.11 | Fact identifiers, ceremony context |
| Multibase | `multibase` 0.9 | base58btc with the `z` prefix; the `ed25519-pub` multicodec prefix `0xed 0x01` is a constant in `lp-crypto`, not a crate |
| Serialisation | `serde` 1 with `derive` | One model for facts, messages, the store and the control channel |
| CBOR | `ciborium` 0.2 | serde-based, Apache-2.0, maintained. No canonicaliser is needed anywhere: signatures are over bytes as sent (ADR 0035) |
| Async runtime | `tokio` 1 | Required by quinn; `rt-multi-thread`, `net`, `sync`, `time`, `macros`, `fs`, `io-util` |
| SQLite | `rusqlite` 0.40 with `bundled` | Same SQLite on both platforms, no system library (LP-25) |

`minicbor` was passed over for its BlueOak licence and non-serde model. Networking,
discovery and platform crates are LP-28 and LP-29.

**Policy.** Caret requirements on the minor version in `Cargo.toml`; `Cargo.lock` committed;
updates are deliberate commits of their own. `cargo-deny` runs in CI (`impl/deny.toml`):
licences allowed are MIT, Apache-2.0, BSD-2/3, ISC, Zlib, Unicode-3.0 and CC0; advisories
and yanked crates fail the build; wildcard versions and unknown sources are refused;
multiple versions of one crate warn. MSRV stays 1.85.
