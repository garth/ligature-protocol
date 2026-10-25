# Changelog

Every release of the Ligature Protocol reference implementation, newest first, in the
shape of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The version is the one
under `[workspace.package]` in `impl/Cargo.toml`, shared by every crate and the Android
app; `scripts/release.sh` moves the Unreleased section under a version and tags it, and the
release workflow publishes the section as the release notes with the installers.

## [Unreleased]

### Added

- Every release carries `SHA256SUMS` over its files and `SHA256SUMS.minisig`, its
  signature by the project's release key, made with minisign; the install guide has the
  public key and the commands that verify a download (LP-139).
- `lpctl upgrade` installs the latest release on a desktop after verifying its checksum
  file against the release key and the package against the checksum; `lpctl upgrade
  --check` only reports, and every other `lpctl` command run on a terminal says once a
  day when a newer release exists (`LIGATURE_NO_UPDATE_CHECK=1` switches the check off)
  (LP-140).
- The input capability (milestone 7): the daemon core that lets one device's keyboard and
  mouse drive another across a configured screen edge or from a touchpad, `lpctl input
  layout`, `status` and `release`, the layout kept in the store, and in-memory backends
  behind `LIGATURE_FAKE_INPUT=1`; no platform captures or injects yet, so no device
  advertises it (LP-142).
- A Mac captures and injects input (milestone 7): an event tap captures the keyboard and
  mouse when the cursor reaches a mapped edge, posted events drive the Mac from another
  device, and the family is offered while Ligature is allowed under Accessibility and Input
  Monitoring (LP-144).
- A Mac shares its screen (milestone 8): ScreenCaptureKit captures a display, ffmpeg encodes
  it with VideoToolbox, and the share is offered where ffmpeg is installed and runs once
  Ligature is allowed to record the screen (LP-150).
- The screen capability (milestone 8): the daemon core that lists a device's displays,
  negotiates a share, chunks frames within the message bound, drops rather than queues
  and recovers with a keyframe, and ends a share on every path; `lpctl screen displays`,
  `start` (the encoded stream to a file), `stop` and `status`; a synthetic capture behind
  `LIGATURE_FAKE_SCREEN=1`; no platform captures or views yet (LP-148).
- Linux: the keyboard and mouse are captured through the InputCapture portal and libei
  when the pointer crosses a mapped screen edge, and injected through the virtual pointer
  and virtual keyboard protocols by a helper, with text typed through a keyboard of its
  own; a Linux daemon now advertises `input` (LP-143).
- Android: the app is a touchpad and keyboard for one of the person's desktops: one
  finger moves, a tap clicks, a hold drags, two fingers scroll, the field types, with
  Backspace, Enter and right-click buttons; the phone advertises `input` as a provider
  only (LP-145).
- Linux: the screen is shared through the ScreenCast portal (or `wlr-screencopy` where the
  portal is missing) and encoded by ffmpeg, with a "Sharing" notification whose action
  stops it; `lpview <device>` watches another device's screen in a window and, after a
  click, drives it until both Control keys are pressed; a Mac with ffmpeg can watch with
  `lpview` too (LP-149).
- Android: the phone shares its screen with another of the person's devices after the
  user consents in Android's dialog each time, with a Stop sharing action, and a Watch
  screen shows a desktop's screen and drives it by touch: a tap clicks where it lands, a
  drag moves the pointer, two fingers scroll, the field types (LP-151).
- Android: touch on the touchpad and the Watch screen is sent in coalesced batches, so
  the pointer follows the finger instead of replaying every movement (LP-151).

### Changed

- A key sent across a drive carries what it means on the controller's layout as well as its
  position, after Synergy and Input Leap (ADR 0043): the driven device types the
  controller's character with its own keys, modifiers keep their meaning across layouts
  that move them, a shortcut is the same letter's, and a dead key is composed on the
  controller. Building on Linux now needs xkbcommon's development files (LP-161).
- On macOS the daemon runs from its own application bundle, `~/Applications/Ligature.app`,
  and shows other devices' notifications through the user notification centre: the
  permission prompt names Ligature, icons are shown, a dismissal on another device clears
  the copy, and closing a copy on the Mac clears it everywhere. The Script Editor step is
  gone, and `ligatured` is no longer installed in `~/.local/bin` on macOS (LP-94).

### Fixed

- `lpview` skips a picture identical to the last and works out its scaling once per redraw, so
  watching a still screen takes about 1% of a core instead of over half of one (LP-149).
- `lpview` no longer prints ffmpeg's "Broken pipe" errors when its window closes or it is
  stopped with Ctrl+C, which now closes it as the window's close button does (LP-150).
- Linux: a screen share whose capture stalls (seen as PipeWire's stream running out of buffers)
  is started again on the same portal session after four seconds without a frame, so the
  viewer's picture recovers instead of freezing; three stalls in a row end the share (LP-149).
- A share whose capture had stopped sending frames outlived the viewer that closed it, and
  the next viewer was refused as busy; a viewer that goes now ends the share at once
  (LP-148).
- The macOS installer waits for the old launchd agent to be gone before loading the new one,
  and retries the load, so an install or `lpctl upgrade` no longer leaves the daemon stopped
  with only launchctl's "Bootstrap failed: 5"; if it still cannot load, it says so and exits
  non-zero (LP-159).
- After a device's daemon restarted, its peers could refuse its reconnects for up to a
  minute; a dial against a connection older than a crossing now replaces it, and a daemon
  that is stopped closes its connections so its peers reconnect at once (LP-158).
- Android: a paste of a promised part or file no longer holds up every other node call
  while it is fetched, the fetch stops after five minutes or when the reader cancels, the
  node starts and stops off the main thread, and a call that lands after the node has
  stopped gets an error rather than a crash (LP-127).
- Android: text shared to Ligature is shown and confirmed before it goes to every device,
  and only another app's `content:` URIs are opened for sending, never a `file:` path or
  the app's own provider; the identity is excluded from device-to-device transfer and
  cloud backup by data extraction rules (LP-127).
- Android: the status bar follows the dark theme, counts in messages are proper plurals,
  the per-app notification switch has a label for screen readers, and the version code
  distinguishes a pre-release from the release of the same number (LP-127).

- A grant never leaves the device that made it: the pairing handover no longer carries the
  introducer's grants, and a grant a peer sends by gossip or in the ceremony is discarded
  (LP-128).
- The store's quarantined rows heal when a copy that verifies arrives, the open-time
  verification skips rows already quarantined, reads one row at a time, writes its verdicts
  in one transaction and quarantines a row whose index columns disagree with its payload or
  no longer read, instead of refusing to open (LP-128).
- Creating or joining an identity writes its facts and the key file as one step, rolling
  the facts back if the key file cannot be saved, and the key file's directory is synced
  after the rename (LP-128).
- Notification icons and the clipboard cache live under the daemon's runtime directory
  (`Locations`), so two daemons on one machine never share them, and the icons are removed
  when the backend stops; the store's time column now sorts as time (schema version 2,
  brought forward on open) (LP-128).

## [0.1.1-alpha] - 2026-10-23

### Fixed

- A revocation counts only when it verifies against the person it names and that person is
  the owner, is abandoning their own master key, or is revoking one of their own devices; a
  self-signed revocation from anyone else changes nothing (LP-120).
- A device that learns a revocation by gossip closes the session it holds with the revoked
  device at once, and the clipboard and notifications are sent only to peers recognised at
  the time of sending (LP-122).
- A peer the daemon has not recognised reaches nothing but the pairing offer: its other
  streams wait for the daemon's verdict and are reset if it is dropped, and strangers kept
  for a ceremony are dropped when it ends (LP-121).
- A device found by mDNS while already connected is no longer dialled again, which replaced
  the live connection and lost what was in flight on it: a dismissal, a clipboard promise, a
  file offer (LP-117).
- A clipboard value received from another device is recorded as applied before the platform
  clipboard gets it, so a paste or a provider read the moment it lands finds its promises
  (LP-107).

## [0.1.0-alpha] - 2026-10-23

### Added

- Pairing of a person's devices with the proximity ceremony (QR code and a six-digit
  code), revocation, and signed facts gossiped between every pair of devices (LP-24,
  LP-74).
- The clipboard shared between a Linux machine, a Mac and an Android phone: text, HTML,
  images and files, with small content pushed on copy and large content fetched only when
  pasted; the phone receives automatically and sends on a tap (LP-24, LP-74, LP-95).
- Notifications posted on one device shown on the others, with the originating app and
  device, and dismissed everywhere at once (LP-86).
- File transfer between devices over the session itself, several at once, with progress
  and cancel, from `lpctl files` on the desktops and the share sheet on the phone
  (LP-87).
- The desktop daemon as a systemd user service on Linux and a launchd agent on macOS,
  installed by `install.sh` from a release tarball, with state in the platform's data
  directory and a fixed UDP port 47471 (LP-85).
- Release builds for Linux (x86_64 and aarch64), macOS (universal) and Android from a
  version tag, published as a GitHub release (LP-85).
