# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this project is

Ligature Protocol (LP) is an open protocol that binds one person's devices (Linux desktops,
Android, tablets, hubs, iPhones where possible) into a single trusted group so any device
can discover and use the others' capabilities: an open, Apple-Continuity-like ecosystem for
Linux users. Principles: local-first, self-hostable with no required central server,
end-to-end encrypted, built on existing open standards, interoperate rather than replace.

Status is pre-alpha. The spec is a draft, the Rust workspace is a scaffold with no protocol
logic, and the Lexicon namespace is a placeholder.

## Architecture decisions

The authoritative record is `docs/adr/`. Summary, in layer order:

1. **Identity**: AT Protocol DIDs (`did:plc`, `did:web`) with DNS handles; `did:plc`
   rotation keys are the recovery root. (ADR 0002)
2. **Device trust**: per-device keys cross-signed by the user identity, Matrix
   cross-signing model; QR/short-code approval from an existing device; revocation. (ADR 0003)
3. **Encryption**: one MLS (RFC 9420) group per user's device set for all peer traffic;
   OpenMLS planned. Pattern from Germ: atproto identity + MLS + separate transport. (ADR 0004)
4. **Transport**: application-level QUIC (RFC 9000), one connection per device pair, one
   stream per capability, device keys as TLS raw public keys (RFC 7250), connection
   migration. MLS stays end-to-end on top; QUIC/TLS is defence in depth. mDNS/DNS-SD + BLE
   locally. Remotely, three replaceable untrusted helper roles: discovery (signed records
   in a DHT via pkarr, or in the user's DID document/Space), hole-punch coordination, relay
   (default: the user's home hub). Rule: any helper may be operated by anyone, is listed in
   the user's own records, and is never trusted with plaintext or identity. Real-time
   traffic never goes through a PDS. WireGuard/Headscale is an optional non-core
   integration only. (ADR 0011, 0012, 0013; ADR 0005 superseded)
5. **Shared state**: device registry, settings, household permissions as typed records in
   an atproto Space (alpha) on a self-hostable PDS. Secrets only as MLS-encrypted
   payloads. (ADR 0006)
6. **Schemas**: atproto Lexicons under `org.ligatureprotocol.*`, a placeholder namespace
   until the domain is registered. (ADR 0007)
7. **Capabilities**: devices advertise typed capabilities (notifications, clipboard, files,
   remote input, media control, telephony/SMS, displays, sensors); peers discover, request,
   and receive explicit grants. (ADR 0008)
8. **Recovery**: Shamir shares held by guardians, in-person verification (QR + spoken code,
   UWB), mandatory delay with veto from existing devices, guardian liveness checks, share
   refresh. (ADR 0009)
9. **Interop**: bridge to KDE Connect, LocalSend, Matter/Thread, CalDAV/CardDAV, Syncthing;
   iPhone via BLE accessory profiles (ANCS/AMS, HFP/MAP/PBAP) and EU DMA APIs. (ADR 0010)

Changing any of these means a new ADR that supersedes the old one, not an edit.

**Under review** (ADR 0014, Proposed): reference implementation in TypeScript plus
language-neutral conformance test vectors, with a Rust production core (UniFFI) later. Do
not restructure `impl/` until that ADR is confirmed.

## Repository layout

- `docs/spec/NN-*.md`: spec chapters, one per layer plus platform notes and threat model.
  Each section has an intent paragraph and inline open questions. No wire formats yet.
- `docs/adr/`: Nygard-format ADRs (Context / Decision / Consequences), status "Proposed".
- `docs/glossary.md`, `docs/prior-art.md`.
- `lexicons/org/ligatureprotocol/*.json`: Lexicon v1 drafts (device, capability, grant,
  guardian). Spec chapters link to these by relative path.
- `impl/`: Rust cargo workspace, edition 2024. Crates: `lp-core` (shared types, capability
  model), `lp-identity` (DID resolution, cross-signing), `lp-crypto` (MLS wrapper),
  `lp-transport` (QUIC connections, local discovery, remote discovery and relay clients),
  `ligatured` (Linux user daemon, D-Bus, systemd unit in `crates/ligatured/systemd/`),
  `lpctl` (CLI). `integrations/wireguard-integration` is a non-core placeholder.
  Dependency direction: the three layer crates depend only on `lp-core`; binaries depend on
  the layer crates; integrations depend on `lp-core` and never the other way round.
- `.github/workflows/ci.yml`: fmt, clippy, test, markdownlint, Lexicon JSON parse.

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
Lexicon files must parse as JSON; CI checks only well-formedness, not Lexicon schema
validity.

## Conventions

- Code: Apache-2.0 (`LICENSE`). Spec text under `docs/`: CC-BY-4.0 (`LICENSE-SPEC`).
- Spec chapters and Lexicons are marked draft; keep the markers until the relevant ADR is
  accepted. Do not add wire formats or state machines to the spec without an accepted ADR.
- New capability families each need a Lexicon under `lexicons/` and a section in
  `docs/spec/05-capabilities.md`.
- The `org.ligatureprotocol` namespace must be replaced or the domain registered before any
  public release.
- Security reports go through GitHub Security Advisories, not issues (`SECURITY.md`).

## Working tickets

The project's work is tracked in its Alba Ticket workspace (`nordstack.albaticket.com`,
project `LP`), reached through the `alba` MCP server as the member who connected it
(`whoami`). It is a local-scope server
(`claude mcp add --transport http alba https://nordstack.albaticket.com/mcp`); if its tools
are missing, reconnect it with `/mcp`. How a ticket is worked:

- **Every change to the repository has a ticket, and no more tickets than the work needs.**
  A request that changes code, spec, ADRs or Lexicons gets a ticket first (`create_ticket`
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
