---
name: update-dependencies
description: USE when updating Rust crates, the Rust toolchain, the Android Gradle plugins and libraries, the NDK, the JDK, GitHub Actions or the CI tooling. Phases with verification; never commits unless asked.
---

# Update Dependencies

Follow the phases in order and do not skip verification. Never commit unless the user asks;
the commit names its ticket (`LP-NNN Update dependencies`), so ask for the key. Crate
versions are pinned by `Cargo.lock` and "bumps are deliberate commits of their own"
(`impl/Cargo.toml`), so a dependency update is its own commit, apart from feature work.

## Where versions are pinned

| What                                                             | Where                                                                                                    |
| ---------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| Crate ranges, grouped with a reason per group                    | `impl/Cargo.toml` `[workspace.dependencies]` (crates use `workspace = true`)                             |
| Resolved crate versions                                          | `impl/Cargo.lock`, committed                                                                             |
| Minimum Rust                                                     | `impl/Cargo.toml` `[workspace.package] rust-version` (1.89); CI uses `dtolnay/rust-toolchain@stable`     |
| Rust and JDK on the primary machine and for Gradle               | `impl/android/mise.toml` (`rust = "stable"`, `java = "21"`); `cargo` is `mise exec rust@stable -- cargo` |
| Licence and advisory policy                                      | `impl/deny.toml` (allow list, `yanked = "deny"`, advisory ignores with a reason)                         |
| Android Gradle plugin, Kotlin, Compose BOM, AndroidX, JNA, ZXing | `impl/android/gradle/libs.versions.toml`                                                                 |
| Gradle                                                           | `impl/android/gradle/wrapper/gradle-wrapper.properties`                                                  |
| NDK                                                              | `impl/android/app/build.gradle.kts` `ndkVersion` (CI installs that version)                              |
| `compileSdk`, `minSdk` (29), `targetSdk`                         | `impl/android/app/build.gradle.kts`                                                                      |
| GitHub Actions                                                   | `.github/workflows/ci.yml`, `release.yml` (`uses:` majors)                                               |
| CI tooling from npm                                              | `ci.yml`: `ajv-cli@5.0.0`, `ajv-formats@3.0.1`; the markdownlint action's major                          |
| The project's own version                                        | `impl/Cargo.toml` `[workspace.package] version`, moved only by `scripts/release.sh`                      |

## Phase 1: crates within their ranges

From `impl/`, on a clean tree:

```sh
cargo update                      # Cargo.lock to the newest versions the ranges allow
cargo deny check                  # advisories, licences, bans, sources
```

Plain `cargo update`: with `--workspace` it touches only the workspace members' own entries
and moves nothing. It prints each change; keep the list for the report. `cargo-outdated` and
`cargo-edit` are not installed; a range bump is an edit to `[workspace.dependencies]`.

## Phase 2: crate majors

1. List candidates: compare each `[workspace.dependencies]` range with the crate's latest on
   crates.io (`cargo search <name>` or the registry page).
2. For each major candidate, before changing the range: read the release notes for the
   versions skipped, grep the workspace for how the crate is used, and check the crates
   that must move together (`quinn` with `rustls` and its `ring` provider; `wayland-client`
   with `wayland-protocols` and `wayland-protocols-wlr`; `objc2` with `objc2-foundation`
   and `objc2-app-kit`; `uniffi` with the Kotlin it generates and the `bindgen` binary;
   `zbus` with `futures-util`; `ed25519-dalek` with `getrandom` and `sha2`).
3. Apply safe majors by editing the range, then `cargo update`, then the code changes the
   migration needs, in one pass per crate group.
4. Hold back a major that needs real migration work and say which breaking change blocks it.
5. Special cases:
   - **`uniffi`**: regenerate the Kotlin (`./gradlew assembleDebug` does) and check the
     generated API against the app's Kotlin; the `bindgen` feature's binary must still run.
   - **`rusqlite`** bundled: a SQLite bump changes nothing in the schema; `user_version`
     stays, but check `PRAGMA` behaviour notes in the release.
   - **`quinn`/`rustls`**: re-run the ceremony code test (the exporter label and length) and
     the raw-public-key handshake tests; no 0-RTT must remain the default.
   - **`clap`**: `lpctl --help` output is in the README for some commands; check it.
   - **`deny.toml`**: an advisory ignore must still be the crate it names; remove it when the
     dependency is gone.

## Phase 3: Rust toolchain

Stable moves on its own through mise (`mise exec rust@stable`); `rust-version` is the
floor. Raise it only when a crate or a language feature needs it, and say so in the commit.
After a stable release, run clippy: new lints arrive with `-D warnings` and CI will fail on
them before anything else.

## Phase 4: Android

1. **Gradle plugins and libraries**: edit `libs.versions.toml`. A major of the Android
   Gradle plugin, Gradle, the NDK or `compileSdk` is its own ticket with the migration read
   first (LP-130 holds the current set). AGP, Kotlin and the Compose
   compiler move together (the Kotlin Compose plugin version equals Kotlin's); the Compose
   BOM sets the Compose libraries. Read the AGP release notes for the Gradle version it
   needs, then bump `gradle-wrapper.properties` (`./gradlew wrapper --gradle-version X`).
2. **NDK**: bump `ndkVersion` in `app/build.gradle.kts` to the LTS `cargo ndk` supports;
   CI installs that exact version with `sdkmanager`, and the primary machine needs it under
   the SDK (`local.properties` names the SDK).
3. **JDK**: `mise.toml` and the CI `setup-java` step move together; the app targets JVM 17
   bytecode regardless.
4. **SDK levels**: `targetSdk` bumps bring new runtime permission and foreground-service
   rules; read the behaviour changes for the new level against `NodeService`,
   `NotificationListener`, `ClipboardProvider` and the share activities, and record
   anything that changes the app's behaviour in chapter 08 section 3.
5. Build: `./gradlew assembleDebug` from `impl/android`, then install on the phone and run
   the first-run checks of its README.

## Phase 5: GitHub Actions and CI tooling

For each `uses:` in both workflows, check the latest major (`gh api repos/<owner>/<repo>/
releases?per_page=5 --jq '.[].tag_name'`) and the runtime it declares
(`gh api "repos/<owner>/<repo>/contents/action.yml?ref=vN" --jq .content | base64 -d |
grep using`); the runners deprecate Node majors, and a warning in a run's annotations says
which actions lag. Read the breaking notes of each major in between (artifact actions
changed path semantics at v5 and hash checking at v8). Bump `ajv-cli` and `ajv-formats` in
`ci.yml` and the markdownlint action together with the local `npx` versions in CLAUDE.md.
The runner labels (`ubuntu-latest`, `ubuntu-24.04-arm`, `macos-latest`) migrate on
GitHub's schedule; a run's annotations announce it.

## Phase 6: verification

All of these, from `impl/` unless said otherwise, must pass:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo ndk -t arm64-v8a -t x86_64 build -p ligature-android --release     # the Android library
(cd android && ./gradlew assembleDebug)                                   # the APK, bindings regenerated
npx --yes markdownlint-cli2 "**/*.md"                                     # from the repository root
```

A flaky test is looped before it is blamed on an update; the two-node and multi-daemon
tests talk over real sockets and mDNS. If an update breaks something that cannot be fixed
quickly, revert that crate's range and report it as held back.

## Phase 7: report

- Crates moved within ranges (count and the notable ones), majors applied (old to new),
  majors held back and why.
- Toolchain, Android and Actions changes, with every file touched.
- `cargo deny check` result and any new advisory ignore with its reason.
- A changelog line under Unreleased only when an update changes behaviour a user sees.
