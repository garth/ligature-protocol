# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this project is

Ligature Protocol (LP) is an open protocol that binds the devices a person owns and a
household shares (Linux and macOS desktops, Android phones and tablets, hubs; ADR 0033) into
a single trusted group so any device can discover and use the others' capabilities, so that
devices from different makers work together the way one maker's do. Apple phones and
tablets are off the roadmap (chapter 08 section 4). Principles: local-first, self-hostable
with no required central server, end-to-end encrypted, built on existing open standards,
interoperate rather than replace.

Status is alpha, with release packages for Linux, macOS and Android on the GitHub Releases
page (tags `vX.Y.Z-alpha`). The spec is a draft, the Rust workspace implements milestones 1 to 5
(pairing, the clipboard with text, HTML, images and files, notifications and file transfer
between a Linux machine, a Mac and an Android phone on one LAN; the phone receives the
clipboard automatically and sends on a tap, since Android lets only a focused app read it;
large clipboard content is fetched only when pasted; the Mac displays
notifications but cannot read its own; files land in each device's download folder, from
`lpctl files` on the desktops and the share sheet on the phone) and nothing beyond them,
and the `org.ligatureprotocol` type names use a placeholder domain.

## Architecture decisions

The authoritative record is `docs/adr/`. Summary, in layer order:

1. **Identity**: a person is an Ed25519 master signing key (ADR 0027; multibase,
   multicodec-prefixed, `z6Mk…`), created on their first device. A person's display name
   is self-asserted in a signed person fact that also carries the subordinate keys (ADR
   0032); there is no key publication. Knowing a person or joining a group requires the
   in-person ceremony; there is no remote joining (ADR 0030). Every person has master,
   self-signing and user-signing keys (ADR 0031); where the master private key lives and
   which devices are introducers follow the user's requirements. (ADR 0017)
2. **Device trust**: one proximity ceremony (QR + a six-digit short authentication string
   derived from the TLS exporter, ADR 0029; BLE/NFC/UWB optional, never required). An
   introducer signs the verified key with a role: `binding` (my account on a device),
   `principal` (a device I use), `person` (someone I know). One ceremony makes the
   newcomer known to all my devices; pairwise-only is rejected. Headless devices serve a
   pairing web page on the LAN. Revocation is a gossiped signed fact. Devices and people
   are identified by their public keys; there is no `deviceId`; a device's name defaults
   to its host name and the user may override it (ADR 0034). (ADR 0003, 0017)
3. **Encryption**: the pairwise QUIC/TLS 1.3 session with raw public keys is the
   end-to-end layer. Least exposure: no device can decrypt what it does not need, so no
   group key. Nothing is sent before mutual authentication. (ADR 0019)
4. **Transport**: application-level QUIC (RFC 9000), one connection per device pair, one
   stream per capability, device keys as TLS raw public keys (RFC 7250), connection
   migration; quinn over rustls, mutual RPK, rustls default suites, no 0-RTT (ADR 0028).
   ALPN `ligature/1` is the protocol version; every stream is bidirectional and starts with
   a header naming its purpose (`gossip`, `manifest` or a capability type); a message is a
   4-byte big-endian length plus one CBOR map with a `type`, 1 MiB maximum by default
   (ADR 0036). The session is the end-to-end layer (decision 3). mDNS/DNS-SD + BLE locally:
   service `_ligature._udp`, TXT keys `v` and `id` only, `lp-<id>` as instance and host
   name, nothing identifying, `id` a hint never a credential (ADR 0037). Remotely,
   three replaceable untrusted helper roles: discovery (signed records in a DHT via pkarr,
   gossiped relays), hole-punch coordination, relay (default: the user's home hub). Rule:
   any helper may be operated by anyone, is listed in the user's own records, and is never
   trusted with plaintext or identity. Real-time traffic never goes through a server LP
   depends on. WireGuard/Headscale is an optional non-core integration. (ADR 0011, 0012,
   0013)
5. **State**: no shared state, no server. Each device keeps a local store (own keys,
   device records with role signatures, known persons, revocations and abandonments,
   issued grants, group memberships, relays) and gossips signed facts to peers after
   authentication at the start of every session, revocations first. Capabilities are
   live: a manifest on connect, updates during the session, withdrawn on disconnect,
   never persisted. No required off-device state at all. (ADR 0016)
6. **Schemas**: JSON Schema 2020-12 under `schemas/`, one file per type; reverse-DNS type
   names `org.ligatureprotocol.*` (placeholder domain). Keys are multibase strings,
   timestamps RFC 3339, bytes base64url in JSON and byte strings in CBOR. Wire encoding is
   CBOR. Every signed fact travels in an envelope (type, payload bytes, signer, signature
   over `"ligature/fact/1" || type || payload`); verify the bytes as received, then
   decode; never re-encode; no canonical form; decoders ignore unknown fields and schemas
   never set `additionalProperties: false`. An envelope never travels bare: on the gossip
   stream it rides in `gossip.fact`, beside `gossip.have` and `gossip.want`
   (`schemas/message.schema.json`). (ADR 0020, 0025, 0035, 0036)
7. **Capabilities**: devices advertise typed capabilities (notifications, clipboard, files,
   remote input, media control, telephony/SMS, displays, sensors); peers discover, request,
   and receive explicit grants. (ADR 0008)
8. **Loss and compromise**: nothing to recover; rebuild with a new master key, re-pair
   the devices you hold, re-verify with household members, gossip an abandonment of the
   old key. (ADR 0018)
9. **Interop**: bridge to KDE Connect, LocalSend, Matter/Thread, CalDAV/CardDAV, Syncthing;
   iPhone via BLE accessory profiles (ANCS/AMS, HFP/MAP/PBAP) and EU DMA APIs. (ADR 0010)
10. **Shared devices and groups**: capabilities are `user`-scoped (clipboard,
    notifications, files; owned by a user-device binding in that person's device set) or
    `device`-scoped (display, speakers, sensors, input; owned by a device principal). A
    shared device has one principal plus zero or more bindings. Groups (households) have
    a group key held by admins; the membership document is signed by it, versioned, and
    lists members by master key and group-owned devices (a lamp) by principal key, so one
    pairing by an admin makes a device available to every member under group policy. A
    device is registered personal or group at pairing; a tablet or TV is group. A request
    carries the authority of the authenticated person behind it: a group device with
    nobody signed in has group authority only and can never reach a member's user-scoped
    capabilities; data reaches it by push. A person signed in to a group device acts with
    their full authority while the device reports the session present. (ADR 0023)
    Same-person use of user-scoped capabilities is implicit; anything across people needs
    approval by the capability's owner or, for device-scoped, by the person present on the
    device, else device policy. `share` is the AirDrop-like family: user-scoped, visible
    only to known persons and group members, accepted per item. Manifests are filtered by
    audience. Cross-person traffic is an ordinary pairwise session. (ADR 0015, 0021, 0022,
    0023)

Changing any of these means a new ADR that supersedes the old one, not an edit.

The reference implementation is Rust and is also the production implementation
(ADR 0026, Accepted). Conformance is defined by the spec and the CBOR test vectors.

## Repository layout

- `docs/spec/NN-*.md`: spec chapters, one per layer plus platform notes, threat model and
  shared devices/groups (chapter 10).
  Each section has an intent paragraph and inline open questions; the wire formats that
  exist (the envelope, the stream header and frame, gossip, pairing and the capability
  families) are those with an ADR behind them.
- `docs/adr/`: Nygard-format ADRs (Context / Decision / Consequences). Superseded ones
  live in `docs/adr/superseded/`.
- `docs/glossary.md`, `docs/prior-art.md`.
- `docs/security-audit/`: dated reports from the `security-audit` skill, the latest linked
  from `SECURITY.md` and the README; a Critical or High finding is named in one only once
  it is fixed or has an advisory.
- `docs/milestones/`: one plan per milestone with scope, non-goals, decisions to pin, exit
  criteria and a task breakdown; milestones 1 to 5 are done and 6 to 8 are drafted (epics
  LP-111, LP-112 and LP-113), none in progress. `01-clipboard-poc.md` is done (Linux and
  macOS share the clipboard) and `01-implementation-decisions.md` records its non-protocol
  choices (store is
  SQLite via rusqlite bundled; keys in a separate 0600 file; state under `data/` in the
  project root during milestone 1, gitignored, and in the platform data directory since
  LP-85; no configuration file; `lpctl init --name` creates the first identity).
  `02-android-clipboard.md` is done, every exit criterion run on a LineageOS phone with the
  two desktops, and `02-implementation-decisions.md`
  records the Android choices (the daemon as a library behind UniFFI; the clipboard read by
  the app when Android allows and automatic only with `READ_CLIPBOARD_IN_BACKGROUND`, which
  only a platform-signed build holds and an official LineageOS build cannot be; a
  `connectedDevice` foreground service with the multicast lock). `03-notifications.md`
  (notifications flow between every device that emits or displays them: one family with
  two directions, dismissal fanned out directly, Linux emitting by monitoring the session
  bus, macOS display-only) has `03-implementation-decisions.md` for its non-protocol
  choices (the `Notifications` trait and `NotificationSync`, `zbus` 6 over the session bus
  with a monitor connection, `lpctl notify`). `04-files.md` is done (file
  transfer native to the session: a control stream per transfer with an offer and an
  accept, a data stream per file with chunks within the 1 MiB frame and a hash at the end,
  the receiver's `maxStreams` bounding how many run at once, cancel by stream reset, a
  receiver-started fetch by an id the sender offered, a standalone file descriptor for
  milestone 5's clipboard to reference) and `04-implementation-decisions.md` records its
  non-protocol choices (`FilesSync` with `FileStore` and `Landing`, part files renamed after
  the hash check, MediaStore pending rows written through `/proc/self/fd` on Android, one
  progress notification per transfer); it is done, its run with the real devices recorded on
  LP-87. `05-rich-clipboard.md` is done (a `set` of several representations,
  pushed within the receiver's `maxBytes` and announced above it as `files` descriptors
  fetched only on paste, files always announced, `get` for the current value, the echo rule
  over the set) and `05-implementation-decisions.md` records its non-protocol choices
  (`Value`, `Incoming` with promises and a `Fetch`, the fingerprint echo rule, the Wayland
  helper's own data-control source, NSPasteboard data providers, the Android content
  provider and Save action); its run with the real devices is recorded on LP-95.
  `06-phone-and-media.md` (the phone's SMS and calls, media control), `07-shared-input.md`
  (one keyboard and mouse across the desktops, the phone as a touchpad) and
  `08-remote-desktop.md` (a screen viewed, then controlled) are drafts.
- `schemas/*.schema.json`: JSON Schema 2020-12 drafts (person, device, capability, grant,
  group, role, revocation, envelope, message, pairing, capability.clipboard,
  capability.notifications, capability.files; `control` is implementation-only).
  Spec chapters link to these by relative path.
- `impl/`: Rust cargo workspace, edition 2024. Crates: `lp-core` (shared types, capability
  model), `lp-identity` (master key, ceremony, roles, trust store; no publication, ADR
  0032), `lp-crypto` (signing, raw-public-key TLS material),
  `lp-transport` (QUIC connections, local discovery; the remote discovery and relay
  clients are unimplemented stubs),
  `ligatured` (user daemon; control socket; systemd unit in `crates/ligatured/systemd/`,
  launchd plist in `crates/ligatured/launchd/`),
  `lpctl` (CLI), `lp-vectors` (conformance vector formats and loader, test support,
  `publish = false`), `ligature-android` (the daemon as a `cdylib` for the Android app behind
  UniFFI 0.32 proc macros, `publish = false`; allows `unsafe_code` for the exported C ABI;
  its `uniffi-bindgen` binary, feature `bindgen`, generates the Kotlin), `lp-macos` (the
  macOS pasteboard behind a safe API, `publish = false`, empty on other platforms; the other
  crate allowing `unsafe_code`, for the Objective-C data provider class).
  `integrations/wireguard-integration` is a non-core placeholder.
  Dependency direction: `lp-crypto` depends only on `lp-core`; `lp-identity` and
  `lp-transport` depend on `lp-core` and `lp-crypto`; binaries depend on the layer crates;
  integrations depend on `lp-core` and never the other way round; `lp-vectors` is only ever
  a dev-dependency; `lpctl` depends on the `ligatured` library for the control channel
  types and client, since the two are built and installed together; `ligature-android`
  depends on the `ligatured` library and runs its loops without the control socket.
- `impl/android/`: the Android app, a Gradle project (Kotlin, Compose, min SDK 29) with its
  own README. Gradle runs `cargo ndk` and `uniffi-bindgen` before compiling; the library and
  the generated bindings live under `app/build/` and are never committed. `mise.toml` there
  names the JDK and Rust; `local.properties` (ignored) names the SDK.
- `vectors/`: conformance vectors (`README.md` has the format and rules; `keys.json` holds
  the committed test keys). Written only by generating tests with `LP_WRITE_VECTORS=1`,
  never by hand, never overwritten.
- `.github/workflows/ci.yml`: fmt, clippy, test on Linux and macOS; the Android library
  cross-build and Kotlin generation; the debug APK; cargo-deny; markdownlint; JSON Schema
  compile (ajv). `release.yml` runs on a `v*` tag: it checks the tag against the workspace
  version, builds the Linux (x86_64, aarch64) and macOS (universal) tarballs with
  `packaging/package.sh` and the signed APK, and publishes a GitHub release with the
  version's `CHANGELOG.md` section as notes.
- `packaging/`: `install.sh` and `uninstall.sh`, shipped in each desktop tarball and usable
  from a checkout (binaries to `~/.local/bin`, the systemd user unit or launchd agent
  enabled), and `package.sh`. `scripts/`: `release.sh LP-NNN X.Y.Z` (bumps the one version
  under `[workspace.package]`, which every crate and the Android app's `versionName` and
  `versionCode` derive from, to `X.Y.Z-alpha`, since every release carries `-alpha` while
  the project is alpha, dates the changelog section, commits and tags `vX.Y.Z-alpha`; never
  pushes),
  `version.sh`, `changelog-notes.sh`. A change worth a release note goes under Unreleased
  in `CHANGELOG.md` with its commit.

## Commands

All cargo commands run from `impl/`. Rust is not installed system-wide on the primary dev
machine; it is provided through `mise`, so prefix with `mise exec rust@stable --` if
`cargo` is not on `PATH`.

```sh
cd impl
cargo build --workspace
cargo test --workspace
cargo test -p lp-core                      # one crate
cargo test -p lp-core capability::         # tests whose path matches
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps      # no rustdoc warnings
```

The daemon and CLI default to the platform locations (`~/.local/share/ligature` and
`$XDG_RUNTIME_DIR/ligature` on Linux, `~/Library/Application Support/ligature` and
`$TMPDIR/ligature` on macOS); `--data-dir` keeps a test daemon apart, and an explicit data
directory puts the socket beside the store. The owner's own daemon on this machine runs as
the systemd user unit from `~/.local/bin`; never kill it from the tool shell.

CI treats clippy warnings as errors and the workspace lints set `missing_docs = "warn"`,
so every public item needs a doc comment. `unsafe_code` is forbidden workspace-wide, except
in `ligature-android`, whose exported C ABI needs it, and `lp-macos`, whose Objective-C
pasteboard provider needs it.

The Android app builds from `impl/android` with the SDK and NDK r27 installed
(`local.properties` names the SDK), the Android Rust targets added and `cargo-ndk`
installed; see `impl/android/README.md`:

```sh
cd impl/android
./gradlew assembleDebug          # cross-builds the Rust library, generates the Kotlin, builds the APK
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

Markdown lint uses `.markdownlint-cli2.jsonc` (100-column lines, licence files ignored) over
every Markdown file in the repository, as CI does: `npx --yes markdownlint-cli2 "**/*.md"`.
Every schema must compile under draft 2020-12 with formats enabled, as CI does:

```sh
npm install --no-save ajv-cli@5.0.0 ajv-formats@3.0.1
npx ajv compile --spec=draft2020 -c ajv-formats -s schemas/<name>.schema.json \
  -r schemas/envelope.schema.json   # -r for every schema the file references
```

## Conventions

- Code: Apache-2.0 (`LICENSE`). Spec text under `docs/`: CC-BY-4.0 (`LICENSE-SPEC`).
- Every diagram is Mermaid in a ```mermaid fence; no plain-text diagrams. A semicolon
  inside a sequence-diagram note or message is a statement separator, so avoid it. Check a
  diagram renders with `npx @mermaid-js/mermaid-cli -p puppeteer.json -i x.mmd -o x.svg`
  where `puppeteer.json` points `executablePath` at `/usr/bin/chromium`.
- Spec chapters and schemas are marked draft; keep the markers until the relevant ADR is
  accepted. Do not add wire formats or state machines to the spec without an accepted ADR.
- New capability families each need a schema under `schemas/` and a section in
  `docs/spec/05-capabilities.md`.
- The `org.ligatureprotocol` type names and the `ligatureprotocol.org` schema `$id`s are
  placeholders, still so in the alpha releases; register the domain or rename before the
  first release that is not alpha.
- Device security is out of scope (ADR 0024): never add lock, idle, local-authentication
  or local-store-protection requirements to the spec. The protocol defines authority (who
  is authenticated behind a request) and trust (which keys are recognised), and takes a
  present session as whatever the device reports.
- The design has no server, no shared database, no group encryption key and nothing
  AT Protocol-specific. Do not reintroduce any of them; the history is in
  `docs/adr/superseded/`. Documentation describes the current design only and never
  mentions superseded ideas.
- Security reports go through GitHub Security Advisories, not issues (`SECURITY.md`).

## Available skills

`.claude/skills/` holds the project's skills, one `SKILL.md` per directory, checked in
(`.claude/settings.local.json` is local and ignored):

- `developer`: the ticket-first, test-first workflow, the project's test shapes, the check
  order and the tool-shell gotchas.
- `qa-engineer`: the protocol's critical paths, the three-platform verification and CI.
- `system-architect`: what needs an ADR, the invariants, crate boundaries, capability design
  and dependency governance.
- `technical-writer`: where each kind of text lives, its shape, the writing conventions,
  Mermaid and markdownlint.
- `update-dependencies`: where every version is pinned and the phases to move crates, the
  toolchain, the Android stack and the workflow actions.
- `security-audit`: a full dated audit or a read-only pre-release gate against the threat
  model and a checklist for a peer-to-peer daemon and its app
  (`/security-audit`, `/security-audit prerelease`).
- `store`: the SQLite trust store's schema, migrations and what never goes in it.
- `android-app`: the app's structure, what Android allows, Compose conventions and driving
  the phone over adb.

## Working tickets

The project's work is tracked in its Alba Ticket workspace (`nordstack.albaticket.com`,
project `LP`), reached through the `alba` MCP server as the member who connected it
(`whoami`). It is a local-scope server
(`claude mcp add --transport http alba https://nordstack.albaticket.com/mcp`); if its tools
are missing, reconnect it with `/mcp`. How a ticket is worked:

- **Every change to the repository has a ticket, and no more tickets than the work needs.**
  A request that changes code, spec, ADRs or schemas gets a ticket first (`create_ticket`
  in `LP`, labelled `claude` when Claude raised it), whose description says where the
  change goes and how it is checked; work already done gets its ticket and is resolved
  straight after the commit. Be conservative about raising one: when a recent ticket's work
  is tried out and found to need a change, reopen that ticket (Reopen, then In Progress,
  with a comment saying what was found) and commit under its key, rather than raising a new
  one. A new ticket is for a change to work committed more than a few days ago, or one that
  reaches parts of the project the original ticket did not.
- **Take it before touching it.** Assign the ticket (`assign_ticket`) and move it to
  In Progress (`transition_ticket`) before reading or editing anything: the board is read
  as the state of the work. Pausing one to take another moves it back to Open with a
  comment saying why.
- **Every commit names its ticket**, the key first in the message:
  `LP-7 Revise transport chapter around QUIC`, on whichever branch is checked out. A ticket
  is usually one commit, and takes more when the work needs them, such as a correction
  after review; two tickets' changes are never in one commit, so stage them separately.
  Commit messages carry no attribution trailer (no `Co-Authored-By` line).
- **`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo test --workspace` pass in `impl/`, and markdownlint passes on the docs**,
  before the commit.
- **Resolve with a comment** (`transition_ticket` to Resolved, Done) that says what was
  done, what was checked and what was not. It does not name the commit: the GitHub
  integration links commits that mention the key to the ticket. A ticket that turns out not
  to be LP's problem is Closed as Declined with the finding.
- **Never push without being asked**, every time; earlier approval does not carry over.
- **Tidy up when done.** Whatever the work made for itself goes once its commits are on the
  branch they were for: worktrees and their local branches, scratch files and processes
  left running. Check first that nothing on it is lost (`git cherry` shows no commit
  missing, the worktree has nothing uncommitted).
