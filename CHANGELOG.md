# Changelog

Every release of the Ligature Protocol reference implementation, newest first, in the
shape of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The version is the one
under `[workspace.package]` in `impl/Cargo.toml`, shared by every crate and the Android
app; `scripts/release.sh` moves the Unreleased section under a version and tags it, and the
release workflow publishes the section as the release notes with the installers.

## [Unreleased]

### Fixed

- A device found by mDNS while already connected is no longer dialled again, which replaced
  the live connection and lost what was in flight on it: a dismissal, a clipboard promise, a
  file offer (LP-117).
- A clipboard value received from another device is recorded as applied before the platform
  clipboard gets it, so a paste or a provider read the moment it lands finds its promises
  (LP-107).

## [0.1.0-alpha] - 2026-10-23

### Added

- Pairing of a person's devices with the proximity ceremony (QR code and a six-digit
  code), revocation, and signed facts gossiped between every pair of devices (milestones
  1 and 2).
- The clipboard shared between a Linux machine, a Mac and an Android phone: text, HTML,
  images and files, with small content pushed on copy and large content fetched only when
  pasted; the phone receives automatically and sends on a tap (milestones 1, 2 and 5).
- Notifications posted on one device shown on the others, with the originating app and
  device, and dismissed everywhere at once (milestone 3).
- File transfer between devices over the session itself, several at once, with progress
  and cancel, from `lpctl files` on the desktops and the share sheet on the phone
  (milestone 4).
- The desktop daemon as a systemd user service on Linux and a launchd agent on macOS,
  installed by `install.sh` from a release tarball, with state in the platform's data
  directory and a fixed UDP port 47471 (LP-85).
- Release builds for Linux (x86_64 and aarch64), macOS (universal) and Android from a
  version tag, published as a GitHub release (LP-85).
