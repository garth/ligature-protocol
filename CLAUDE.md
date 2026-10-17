# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this project is

Ligature Protocol (LP) is an open protocol that binds one person's devices (Linux desktops,
Android, tablets, hubs, iPhones where possible) into a single trusted group so any device
can discover and use the others' capabilities: an open, Apple-Continuity-like ecosystem for
Linux users. Principles: local-first, self-hostable with no required central server,
end-to-end encrypted, built on existing open standards, interoperate rather than replace.

Status is pre-alpha. The spec is a draft, the Rust workspace is a scaffold with no protocol
logic, and the `org.ligatureprotocol` type names use a placeholder domain.

## Architecture decisions

The authoritative record is `docs/adr/`. Summary, in layer order:

1. **Identity**: a person is a master signing key (multibase, multicodec-prefixed),
   created on their first device. Publishing it at a DNS name (TXT or well-known HTTPS
   path) is optional, for a name and remote invitations. Key configuration (split
   master/self-signing/user-signing vs one key; which devices are introducers) follows the
   user's requirements. Nothing AT Protocol-specific remains. (ADR 0017; ADR 0002
   superseded)
2. **Device trust**: one proximity ceremony (QR + spoken code baseline; BLE/NFC/UWB
   optional, never required). An introducer signs the verified key with a role: `binding`
   (my account on a device), `principal` (a device I use), `person` (someone I know). One
   ceremony makes the newcomer known to all my devices; pairwise-only is rejected.
   Headless devices serve a pairing web page on the LAN. Revocation is a gossiped signed
   fact. (ADR 0003, 0017)
3. **Encryption**: the pairwise QUIC/TLS 1.3 session with raw public keys is the
   end-to-end layer. Least exposure: no device can decrypt what it does not need, so no
   group key; MLS is only a candidate for a future capability-scoped multicast key.
   Nothing is sent before mutual authentication. (ADR 0019; ADR 0004 superseded)
4. **Transport**: application-level QUIC (RFC 9000), one connection per device pair, one
   stream per capability, device keys as TLS raw public keys (RFC 7250), connection
   migration. The session is the end-to-end layer (decision 3). mDNS/DNS-SD + BLE
   locally. Remotely, three replaceable untrusted helper roles: discovery (signed records
   in a DHT via pkarr, gossiped relays, optional published key), hole-punch coordination, relay
   (default: the user's home hub). Rule: any helper may be operated by anyone, is listed in
   the user's own records, and is never trusted with plaintext or identity. Real-time
   traffic never goes through a PDS. WireGuard/Headscale is an optional non-core
   integration only. (ADR 0011, 0012, 0013; ADR 0005 superseded)
5. **State**: no shared state, no server. Each device keeps a local store (own keys,
   device records with role signatures, known persons, revocations and abandonments,
   issued grants, group memberships, relays) and gossips signed facts to peers after
   authentication at the start of every session, revocations first. Capabilities are
   live: a manifest on connect, updates during the session, withdrawn on disconnect,
   never persisted. No required off-device state at all. (ADR 0016; ADR 0006 superseded)
6. **Schemas**: JSON Schema 2020-12 under `schemas/`, one file per type; reverse-DNS type
   names `org.ligatureprotocol.*` (placeholder domain). Keys are multibase strings,
   timestamps RFC 3339, bytes base64url. Wire encoding still open. (ADR 0020; ADR 0007
   superseded)
7. **Capabilities**: devices advertise typed capabilities (notifications, clipboard, files,
   remote input, media control, telephony/SMS, displays, sensors); peers discover, request,
   and receive explicit grants. (ADR 0008)
8. **Loss and compromise**: nothing to recover; rebuild with a new master key, re-pair
   the devices you hold, re-verify with household members, gossip an abandonment of the
   old key. Social recovery (ADR 0009) is documented as an optional extension only.
   (ADR 0018; ADR 0009 superseded)
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
    their full authority while the device reports the session present. (ADR 0023, items 4
    and 5 superseded by 0024)
    Same-person use of user-scoped capabilities is implicit; anything across people needs
    approval by the capability's owner or, for device-scoped, by the person present on the
    device, else device policy. `share` is the AirDrop-like family: user-scoped, visible
    only to known persons and group members, accepted per item. Manifests are filtered by
    audience. Cross-person traffic is an ordinary pairwise session. (ADR 0015, 0021, 0022,
    0023)

Changing any of these means a new ADR that supersedes the old one, not an edit.

**Under review** (ADR 0014, Proposed): reference implementation in TypeScript plus
language-neutral conformance test vectors, with a Rust production core (UniFFI) later. Do
not restructure `impl/` until that ADR is confirmed.

## Repository layout

- `docs/spec/NN-*.md`: spec chapters, one per layer plus platform notes, threat model and
  shared devices/groups (chapter 10).
  Each section has an intent paragraph and inline open questions. No wire formats yet.
- `docs/adr/`: Nygard-format ADRs (Context / Decision / Consequences), status "Proposed".
- `docs/glossary.md`, `docs/prior-art.md`.
- `schemas/*.schema.json`: JSON Schema 2020-12 drafts (device, capability, grant, group).
  Spec chapters link to these by relative path.
- `impl/`: Rust cargo workspace, edition 2024. Crates: `lp-core` (shared types, capability
  model), `lp-identity` (master key, ceremony, roles, trust store, optional publication),
  `lp-crypto` (signing, raw-public-key TLS material),
  `lp-transport` (QUIC connections, local discovery, remote discovery and relay clients),
  `ligatured` (Linux user daemon, D-Bus, systemd unit in `crates/ligatured/systemd/`),
  `lpctl` (CLI). `integrations/wireguard-integration` is a non-core placeholder.
  Dependency direction: the three layer crates depend only on `lp-core`; binaries depend on
  the layer crates; integrations depend on `lp-core` and never the other way round.
- `.github/workflows/ci.yml`: fmt, clippy, test, markdownlint, JSON Schema compile (ajv).

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
npx ajv compile --spec=draft2020 -c ajv-formats -s schemas/<name>.schema.json
```

## Conventions

- Code: Apache-2.0 (`LICENSE`). Spec text under `docs/`: CC-BY-4.0 (`LICENSE-SPEC`).
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
- Nothing AT Protocol-specific is used. Do not reintroduce DIDs, Lexicons, NSIDs, PDS or
  Spaces; the history is in ADRs 0002, 0006, 0007 and their successors.
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
