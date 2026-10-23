---
name: developer
description: USE when implementing features or fixing bugs in the Rust workspace or the Android app. Enforces the ticket-first workflow, test-first discipline, the check order and the project's test shapes.
---

# Developer

## Before touching anything

- Every change has a ticket in Alba (`LP`), taken (assigned, In Progress) before the first edit;
  a recent ticket whose work needs a change is reopened rather than duplicated. CLAUDE.md
  "Working tickets" is the rule; this skill does not restate it.
- Read the milestone plan the work belongs to (`docs/milestones/NN-*.md`) and the
  `NN-implementation-decisions.md` beside it. A non-protocol choice made while implementing is
  recorded there, in the same commit.
- A protocol change (wire format, message, trust rule, capability shape) needs an accepted ADR
  first. If the task implies one, stop and say so; do not implement around it.

## Test first

- Start from a failing test that states the behaviour, then the minimum code that passes it.
- A bug fix starts with a test that reproduces it. For a flaky test, reproduce by looping it
  (`for i in $(seq 1 15); do cargo test -p <crate> --test <name>; done`) with the daemons'
  debug logging on, and read the failing run's log before changing anything: the cause has been
  a race in the daemon every time so far, never the test.
- All tests pass before a task is complete. A failing test is never "another change's": find
  the cause. An obsolete test is removed with a sentence saying why.

## The project's test shapes

- **Unit tests** live in the module (`#[cfg(test)] mod tests`), with no I/O; `lp-core::paths`
  takes its environment as a closure for exactly this reason.
- **Two- or three-daemon tests** in `impl/crates/ligatured/tests/`: each daemon on
  `127.0.0.1:0` in a `tempfile` directory, with the in-memory backends (`FakeNotifications`,
  the in-memory `Clipboard`, a fake `FileStore`), persons and roles made with
  `lp_identity::facts`, and an `until(what, check)` helper with a bounded timeout. They talk
  over real QUIC and real mDNS, so they must tolerate the owner's own daemon and other
  devices on the LAN (seen as strangers and dropped) and a host with several addresses.
- **The Android two-node test** (`impl/crates/ligature-android/tests/two_nodes.rs`) drives
  the UniFFI node with a `FakeHost`; it is the integration test of the whole daemon.
- **Conformance vectors** under `vectors/` are generated, never hand-written: a generating
  test runs with `LP_WRITE_VECTORS=1`, and an existing vector is never overwritten. Every
  implementation must verify the bytes as received (ADR 0035), so a test that re-encodes and
  compares is a reference-implementation extra, not a conformance requirement.
- Something a real platform does (a clipboard manager that reads every selection, a
  pasteboard that asks on the main thread) is recorded in the run on the milestone's epic
  and in chapter 08, not simulated in a test unless it can be.

## Code rules the lints will catch, and some they will not

- Every public item has a doc comment (`missing_docs = "warn"`, and CI runs clippy with
  `-D warnings`). Doc comments say what the thing is for, in sentences.
- `unsafe_code` is forbidden except in `ligature-android` (the C ABI) and `lp-macos` (the
  Objective-C provider); never add a third.
- Dependency direction between crates is fixed (CLAUDE.md "Repository layout"); a new
  dependency goes in `[workspace.dependencies]` with a one-line reason and is pinned by
  `Cargo.lock`.
- Platform code is target-gated in `ligatured` (`clipboard_linux.rs`, `clipboard_macos.rs`,
  `notifications_*`); library crates never depend on a platform crate.
- Decoders ignore unknown fields and schemas never set `additionalProperties: false`; a
  signed payload is verified as bytes and never re-encoded.
- Logging is `tracing`: `info` for what an operator wants to see once per event, `debug` for
  the mechanism, `warn` for something that needs attention. A debug line that would have
  shortened the last investigation is worth adding.
- British spelling in prose, comments and docs, as the existing text has it.

## Check order

From `impl/`, before every commit (Rust is through mise on the primary machine:
`mise exec rust@stable -- cargo …`):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

and on the docs, from the repository root:

```sh
npx --yes markdownlint-cli2 "*.md" "docs/**/*.md"
```

A change to a schema also needs the ajv compile from CLAUDE.md; a change to the Android app
also needs `./gradlew assembleDebug` from `impl/android` (or at least `./gradlew -q --offline
:app:tasks` for a Gradle-only change). A change worth a release note goes under Unreleased in
`CHANGELOG.md` in the same commit.

## Running daemons from the tool shell

- `cargo` on `PATH` is a mise shim with no global default; use `mise exec rust@stable --`.
- Never `pkill -f <pattern>` or `pgrep -f` from the tool shell: the pattern matches the
  shell's own command line and kills the call. Record PIDs and kill those.
- The owner's own daemon runs on this machine with the owner's clipboard; never kill it, and
  never test with the real Wayland clipboard (a nested Hyprland on its own socket gives an
  isolated seat). Control sockets need short paths (`LIGATURE_RUNTIME_DIR=/tmp/lp-x`).
- A rebuild of a binary a running daemon spawns helpers from breaks its respawn until the
  owner restarts it.
