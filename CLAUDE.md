# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this project is

Ligature Protocol (LP) is an open protocol that binds one person's devices (Linux and
macOS desktops, Android, tablets, hubs, iPhones where possible; ADR 0033) into a single
trusted group so any device can discover and use the others' capabilities: an open, Apple-
Continuity-like ecosystem for Linux users. Principles: local-first, self-hostable with no
required central server, end-to-end encrypted, built on existing open standards,
interoperate rather than replace.

Status is pre-alpha. The spec is a draft, the Rust workspace is a scaffold with no protocol
logic, and the `org.ligatureprotocol` type names use a placeholder domain.

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
  Each section has an intent paragraph and inline open questions. No wire formats yet.
- `docs/adr/`: Nygard-format ADRs (Context / Decision / Consequences). Superseded ones
  live in `docs/adr/superseded/`.
- `docs/glossary.md`, `docs/prior-art.md`.
- `docs/milestones/`: one plan per milestone with scope, non-goals, decisions to pin, exit
  criteria and a task breakdown; `01-clipboard-poc.md` is the current one, and
  `01-implementation-decisions.md` records non-protocol implementation choices (store is
  SQLite via rusqlite bundled; keys in a separate 0600 file; state under `data/` in the
  project root during milestone 1, gitignored; no configuration file; `lpctl init --name`
  creates the first identity).
- `schemas/*.schema.json`: JSON Schema 2020-12 drafts (person, device, capability, grant,
  group, role, revocation, envelope, message; `control` is implementation-only).
  Spec chapters link to these by relative path.
- `impl/`: Rust cargo workspace, edition 2024. Crates: `lp-core` (shared types, capability
  model), `lp-identity` (master key, ceremony, roles, trust store, optional publication),
  `lp-crypto` (signing, raw-public-key TLS material),
  `lp-transport` (QUIC connections, local discovery, remote discovery and relay clients),
  `ligatured` (Linux user daemon, D-Bus, systemd unit in `crates/ligatured/systemd/`),
  `lpctl` (CLI), `lp-vectors` (conformance vector formats and loader, test support,
  `publish = false`). `integrations/wireguard-integration` is a non-core placeholder.
  Dependency direction: the three layer crates depend only on `lp-core`; binaries depend on
  the layer crates; integrations depend on `lp-core` and never the other way round;
  `lp-vectors` is only ever a dev-dependency.
- `vectors/`: conformance vectors (`README.md` has the format and rules; `keys.json` holds
  the committed test keys). Written only by generating tests with `LP_WRITE_VECTORS=1`,
  never by hand, never overwritten.
- `.github/workflows/ci.yml`: fmt, clippy, test on Linux and macOS; markdownlint; JSON
  Schema compile (ajv).

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
```

CI treats clippy warnings as errors and the workspace lints set `missing_docs = "warn"`,
so every public item needs a doc comment. `unsafe_code` is forbidden workspace-wide.

Markdown lint uses `.markdownlint-cli2.jsonc` (100-column lines, license files ignored).
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
  placeholders; register the domain or rename before any public release.
- Device security is out of scope (ADR 0024): never add lock, idle, local-authentication
  or local-store-protection requirements to the spec. The protocol defines authority (who
  is authenticated behind a request) and trust (which keys are recognised), and takes a
  present session as whatever the device reports.
- The design has no server, no shared database, no group encryption key and nothing
  AT Protocol-specific. Do not reintroduce any of them; the history is in
  `docs/adr/superseded/`. Documentation describes the current design only and never
  mentions superseded ideas.
- Security reports go through GitHub Security Advisories, not issues (`SECURITY.md`).

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
