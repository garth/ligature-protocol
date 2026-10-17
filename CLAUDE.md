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
4. **Transport**: direct device-to-device. mDNS/DNS-SD + BLE locally; Headscale-compatible
   WireGuard overlay remotely. Real-time traffic never goes through a PDS. (ADR 0005)
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

## Repository layout

- `docs/spec/NN-*.md`: spec chapters, one per layer plus platform notes and threat model.
  Each section has an intent paragraph and inline open questions. No wire formats yet.
- `docs/adr/`: Nygard-format ADRs (Context / Decision / Consequences), status "Proposed".
- `docs/glossary.md`, `docs/prior-art.md`.
- `lexicons/org/ligatureprotocol/*.json`: Lexicon v1 drafts (device, capability, grant,
  guardian). Spec chapters link to these by relative path.
- `impl/`: Rust cargo workspace, edition 2024. Crates: `lp-core` (shared types, capability
  model), `lp-identity` (DID resolution, cross-signing), `lp-crypto` (MLS wrapper),
  `lp-transport` (mDNS/BLE/connections), `ligatured` (Linux user daemon, D-Bus, systemd
  unit in `crates/ligatured/systemd/`), `lpctl` (CLI). Dependency direction: the three
  layer crates depend only on `lp-core`; binaries depend on the layer crates.
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
