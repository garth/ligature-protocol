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

**Locations.** During milestone 1 the data directory is `data/` under the project root,
ignored by git (LP-32). `$XDG_DATA_HOME/ligature/` (default `~/.local/share/ligature/`) on
Linux and `~/Library/Application Support/ligature/` on macOS become the defaults afterwards.
One function (`lp-core::paths::Locations`) resolves it, overridable with `--data-dir`.

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
| Randomness | `getrandom` 0.4 | Operating-system randomness for key seeds; `ed25519-dalek` 3 builds keys from a 32-byte seed, and its `rand_core` 0.10 has no OS generator of its own |
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

## LP-30: control channel between lpctl and ligatured

Not protocol: the channel exists between two processes on one machine and nothing on it
crosses to another device.

**Socket.** `<runtime-dir>/control.sock`. During milestone 1 the runtime directory is the
data directory, so the socket is `data/control.sock` under the project root (LP-32).
Afterwards it becomes `$XDG_RUNTIME_DIR/ligature/` on Linux and `$TMPDIR/ligature/` on
macOS (the per-user temporary directory launchd agents receive, short enough for the
104-byte socket path limit), falling back to the data directory if the variable is unset.
`--runtime-dir` overrides it (G2 runs two daemons). Directory 0700, socket 0600. On start the daemon
removes a stale socket file once connecting to it fails. On accept it reads the peer
credentials and drops a connection whose uid is not its own.

**Framing.** A frame is a 4-byte big-endian length followed by exactly one CBOR map. A
length above 1 MiB closes the connection before anything is decoded. Unknown fields are
ignored. Nothing on this channel is signed, so the byte-exact rule of ADR 0035 does not
arise. QUIC streams use the same frame
([ADR 0036](../adr/0036-stream-framing-and-message-typing.md)), so one `Frame` codec in
`lp-core` serves both.

**Messages.** [`schemas/control.schema.json`](../../schemas/control.schema.json), type
names `org.ligatureprotocol.control.*`, marked implementation-only and linked from no spec
chapter. Three shapes, each a map with `type` and `id`: a **request** from `lpctl` (`id`
chosen by `lpctl`, unique on the connection), exactly one **response** per request (same
`id`, `ok` with a result or `error` with a code and message), and zero or more **events**
before the response (same `id`). One `lpctl` invocation is one connection; the daemon
accepts many and answers each connection's requests in order.

**Request set.** `status`, `devices`, `caps`, `revoke` (by key), and the ceremony:
`pair.introduce` (with the scanned offer) and `pair.join` open the two sides and stay open
while the ceremony runs. The daemon sends `pair.offer` (the text `lpctl` renders as a QR
code) and `pair.code` (the six digits) as events; the user's confirmation is a second
request on the same connection, `pair.confirm` or `pair.abort`, naming the open pairing's
`id`; the final response closes the ceremony. Long-running operations therefore need no
second channel.

**Versioning.** None on the wire. `status` returns the daemon's version; `lpctl` and
`ligatured` come from one workspace and are installed together, and an unknown request type
answers with the error code `unknown-type`.

## LP-31: stream framing, message typing and versioning

Became [ADR 0036](../adr/0036-stream-framing-and-message-typing.md) because peers must
agree on it: length-prefixed typed CBOR frames, a stream header as the first frame, gossip
messages carrying envelopes, ALPN `ligature/1` as the only version. Implementation note: the
`Frame` codec and the message types live in `lp-core`; `lp-transport` dispatches streams on
the header and knows nothing about what the messages mean.

## LP-32: configuration, first run and locations

**No configuration file.** The daemon has three inputs: the data directory, the runtime
directory and the log level. Everything else is a fact in the store or an argument to an
`lpctl` command.

**Directories.** `--data-dir` and `--runtime-dir` on both binaries, with `LIGATURE_DATA_DIR`
and `LIGATURE_RUNTIME_DIR` as their defaults, so a script sets two variables once and every
process it starts agrees. For milestone 1 the data directory defaults to `data/` relative
to the working directory, which is the project root when run from a checkout, and the
runtime directory defaults to the data directory; `data/` is ignored by git. G2 runs its two
daemons in `data/a` and `data/b`. The platform locations named under LP-25 and LP-30 take
over after the milestone, by changing the defaults in `lp-core::paths` and nothing else.

**First run.** A daemon starts with no identity and serves only its control socket:
`status` reports `initialised: false`, nothing is advertised on mDNS, and the only other
request it accepts is a pairing started by `lpctl pair`. `lpctl init --name "Garth"` creates
the master, self-signing and user-signing keys and this device's binding and principal keys
in one step; the person's name is required because it has no sensible default (ADR 0032),
the device's name defaults to the host name (ADR 0034) and `--device-name` overrides it.
`lpctl pair` as a newcomer creates device keys only and receives the person through the
ceremony, so a device never becomes a person by accident. Renaming is a later request.

**Logs.** stderr (LP-29). The systemd user unit needs nothing; the launchd plist (F4) sets
`StandardErrorPath` to `~/Library/Logs/ligature/ligatured.log`. No rotation in milestone 1.

## LP-33: conformance vector format and test layout

Not protocol, but the format other implementations test against, so fixed from the first
vector. Full rules in [`vectors/README.md`](../../vectors/README.md).

**Bytes, not encodings.** A vector carries the exact `payload`, `signature` and `factId`
the reference produced and asks an implementation to verify them, decode the payload to the
given `input`, and reproduce the signature from the named seed. Reproducing the payload
bytes is optional, as ADR 0035 requires. The ticket's wording about "deterministic CBOR"
predates that ADR and is withdrawn.

**Layout.** `vectors/keys.json` (six named test keys with committed seeds, test material
only), then `keys/`, `facts/`, `ceremony/`, `gossip/`, one JSON file per case, negative
cases included. Byte fields are hex; `input` follows the schema; keys are multibase.

**Loader.** The workspace crate `lp-vectors` (`publish = false`) holds the exact Rust
shapes (`KeyFile`, `FactVector`, `CodeVector`), `load_dir`, `load_file` and `write_file`.
Layer crates use it as a dev-dependency only; it is never in a binary. Generation is a test
that runs only with `LP_WRITE_VECTORS=1` and refuses to overwrite, so every change to a
vector is a deliberate, reviewed diff. Every ordinary test run loads and checks every file.

## LP-34: how the Linux daemon reaches the Wayland clipboard

Not protocol. `wlr-data-control` needs a connection to the user's compositor, and a systemd
user service only sees `WAYLAND_DISPLAY` when something imported it (UWSM does; a bare
Hyprland session usually does not) and only if it started after the compositor.

**Decision.** The clipboard provider takes the socket from `WAYLAND_DISPLAY` when set, else the
newest `wayland-N` socket (not `.lock`) in `$XDG_RUNTIME_DIR`. `wl-clipboard-rs` has no public way
to take a socket path and setting a process-wide environment variable at runtime is unsafe in Rust,
so the Wayland side runs in a helper child process (`ligatured clipboard-helper`) started with
`WAYLAND_DISPLAY` naming that socket; parent and child exchange the usual length-prefixed CBOR
frames over the child's stdio. If nothing connects, the daemon runs without a clipboard capability
and retries every few seconds; when the compositor goes away it withdraws the capability (live
manifests, ADR 0016) and waits again. A stale socket that refuses the connection is skipped. The
daemon is never started by the compositor and the unit stays `WantedBy=default.target`, so gossip
and revocations work with nobody logged in graphically. Chapter 08 section 2 states the behaviour.

## LP-35: mDNS record contents and the per-boot identifier

Became [ADR 0037](../adr/0037-local-discovery-record.md) because implementations must agree
on the service type and record: `_ligature._udp`, TXT keys `v` and `id` only, `lp-<id>` as
instance and host name, nothing identifying, `id` a hint and never a credential. The
per-epoch recognisable identifier stays an open question in chapter 04. Implementation
notes: `id` is 8 bytes from `getrandom` at daemon start; the browser keeps a map from `id`
to last outcome with a back-off after a failed authentication and a cap on concurrent dials;
each daemon in G2 has its own `id` and port, so two on one machine are distinct instances.
