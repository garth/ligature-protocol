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

## LP-28: crates for networking and discovery

| Need | Crate | Why |
| --- | --- | --- |
| QUIC | `quinn` 0.11 | Decided in ADR 0028; MIT or Apache-2.0; MSRV 1.85. Features `runtime-tokio`, `rustls-ring` and `log` only: no `platform-verifier`, since peers are verified as raw public keys, never against a certificate store |
| TLS 1.3 | `rustls` 0.23 | The line quinn 0.11 builds against (0.24 is a prerelease). Features `ring`, `std`, `logging`; no `tls12`, no `aws_lc_rs` |
| Crypto provider | `ring` through rustls | Builds without cmake or a C toolchain beyond cc on both platforms. It covers TLS 1.3 with Ed25519 and the rustls default suites. `aws-lc-rs` would only matter for FIPS or post-quantum, neither of which is a goal |
| mDNS/DNS-SD | `mdns-sd` 0.21 | Pure Rust, advertise and browse, Apache-2.0 or MIT, maintained, no Avahi or Bonjour dependency. Runs its own thread and talks over channels, so it fits behind the discovery trait regardless of runtime |

**Raw public keys.** rustls 0.23 supports RFC 7250 through `CertificateType::RawPublicKey`
and custom verifiers (`ServerCertVerifier`, `ClientCertVerifier`) that receive the
`SubjectPublicKeyInfo` bytes. Task B1 (LP-41) is the gate that proves the surface works for
both roles on both platforms, with the exporter and migration; this decision does not
pre-empt it.

**macOS.** The same `mdns-sd` stack runs on both platforms. `mdns-sd` binds port 5353 with
address reuse and coexists with mDNSResponder in common use, but B3 (LP-43) must show it on
a real Mac. If it fails, `astro-dnssd` (bindings to the system DNS-SD API: Bonjour on macOS,
Avahi's compatibility layer on Linux) is the macOS implementation behind the same trait.
`zeroconf` was passed over for an unknown licence on crates.io, `libmdns` because it only
advertises.

**UDP port.** The QUIC endpoint binds port 0 and the operating system assigns one; the
daemon advertises the assigned port in the DNS-SD SRV record. Two daemons on one machine
therefore never collide (G2) and nothing is configured. A fixed `--port` for strict
firewalls is not a milestone 1 need; the flag is reserved and documented under LP-32 when
it is added.

**Pinning.** The crates are in the workspace now so `Cargo.lock` carries them before B1 and
B3 start. Nothing in the scaffold uses them yet.

## LP-29: crates for platform integration and tooling

| Need | Crate | Why |
| --- | --- | --- |
| Wayland clipboard (E3) | `wl-clipboard-rs` 0.9 | Pure Rust Wayland client speaking `wlr-data-control` and `ext-data-control`, so a daemon with no focused window can watch and set the clipboard. Covers Hyprland, Sway, river, Niri and KDE; GNOME stays behind the trait. `arboard` was passed over: its Wayland path needs a focused window |
| macOS pasteboard (E4) | `objc2` 0.6, `objc2-foundation` 0.3, `objc2-app-kit` 0.3 | Type-safe generated bindings to `NSPasteboard`. The watcher polls `changeCount` (chapter 08), which `arboard` does not expose |
| CLI (C1) | `clap` 4.6 with `derive` | MSRV 1.85 matches ours |
| QR in the terminal (C1) | `qrcode` 0.14, no default features | Renders to Unicode block characters with no image dependency; its SVG output serves the pairing page later |
| Logging | `tracing` 0.1 in every crate, `tracing-subscriber` 0.3 with `env-filter` in the binaries | Structured, async-aware, the quinn and rustls stacks already emit through it |
| Control socket (F2) | tokio `UnixListener` and `ciborium` | Already in the workspace |

**Log sinks.** Both binaries write to stderr, level from `RUST_LOG` with `info` as the
default. systemd captures that into the journal and launchd into the file its plist names,
so `tracing-journald` and `oslog` are not milestone 1 crates; either can be added without
touching the library crates. Where launchd's log file lives is settled under LP-32.

**Placement.** Every crate is a workspace dependency so a version is set once. The clipboard
crates are target-gated in `ligatured` (`cfg(target_os = "linux")` and
`cfg(target_os = "macos")`), so neither platform compiles the other's bindings and CI
exercises both. Library crates (`lp-core`, `lp-identity`, `lp-crypto`, `lp-transport`)
never depend on a clipboard, terminal or UI crate.
