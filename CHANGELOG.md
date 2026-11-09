# Changelog

Every release of the Ligature Protocol reference implementation, newest first, in the
shape of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The version is the one
under `[workspace.package]` in `impl/Cargo.toml`, shared by every crate and the Android
app; `scripts/release.sh` moves the Unreleased section under a version and tags it, and the
release workflow publishes the section as the release notes with the installers.

## [Unreleased]

### Added

- A cast: a share of the `screen` family that the sharing side starts, with `offer` answered
  by the receiver's `start` or `refused` (`declined`, `no-decoder`), and a `receives` param for
  a device that shows casts unasked. `lpctl screen cast` offers a display or a window to a
  device, the daemon opens `lpview --receive` for a cast offered to a desktop, and `lpctl
  screen status` lists casts and the offers waiting (milestone 9, LP-198, LP-199).
- Windows beside displays: `displays` lists the windows a sharer can share by id, `start` may
  name a `window`, and a window share is watched, never driven (LP-198, LP-199).
- Sound with the picture: `audioEncodes` and `audioDecodes` params, `audio` formats in
  `start`, `offer` and `started` (Opus, 48 kHz, mono or stereo), and `audio` packets of 20 ms
  on the share's stream between the frame chunks, on the frames' clock and never dropped for
  flow control; `lpctl screen start --audio-out` records them (LP-198, LP-199). On Linux the
  default output's sound is captured through PipeWire and encoded by ffmpeg's `libopus`,
  `lpview` plays a share's sound through ffmpeg (`--mute` for the picture alone), a cast with
  `--window` and no id picks the window in the portal's picker, and a window that changes
  shape is letterboxed (LP-200). On a Mac the windows ScreenCaptureKit lists are shared by
  id and the system's sound comes with the picture on macOS 13 and later; `lpview` plays
  through AudioToolbox (LP-201). On the phone a Cast card sends its screen to a device that
  shows casts, a cast offered to the phone opens the watch screen or a notification, a
  watched share plays its sound with a Mute, and the phone's own sound goes with its screen
  once the recording permission Android puts it behind is allowed (LP-202).

## [0.2.0-alpha] - 2026-11-08

### Added

- One keyboard and mouse across the desktops (milestone 7): the pointer crosses from one
  desktop to the other at an edge set with `lpctl input layout`, the keyboard follows it, and
  `lpctl input status` and `lpctl input release` show and end a drive (LP-142, LP-143,
  LP-144).
- Keys type what they mean on the controlling keyboard, after Synergy and Input Leap: the
  driven device presses its own key for the controller's character, modifiers keep their
  meaning across layouts, a shortcut is the same letter's, and a dead key is composed on the
  controller; building on Linux needs xkbcommon's development files (ADR 0043, LP-161).
- Driving another device from a Mac, the right Option key types the characters it makes on
  the Mac, accents included, as AltGr does on a PC; the left Option key stays Alt (ADR 0046,
  LP-175).
- Both Control keys together, or an Alt or Super key on each side (Option or Command on a
  Mac), give the keyboard and mouse back (LP-142, LP-171).
- The phone is a touchpad and keyboard for either desktop: one finger moves, a tap clicks, a
  hold drags, two fingers scroll, the field types, with Backspace, Enter and right-click
  buttons (LP-145, LP-156).
- A desktop the phone drives carries the pointer on across its own edges to the next desktop,
  and no device ever drives the device driving it (LP-174).
- The phone is driven from your other devices once Ligature is on under Accessibility: a
  pointer on its screen, clicks as taps, the right button for Back and the middle for Home,
  the wheel to scroll, typing into the field in focus on Android 13 or later, and a locked
  phone unlocked by dragging up and clicking its PIN; the screen wakes for a drive or a watch
  (LP-167).
- What you copy on the phone is sent by itself once Ligature is on under Accessibility, on
  any ROM, a second copy within seconds included; Android shows a toast for each clip read
  unless Show clipboard access is off, and a clip its app marked sensitive is not sent
  (LP-166).
- A screen watched and driven from another device (milestone 8): `lpview <device>` shows
  another device's display in a window and, after a click, drives it from the picture; the
  phone's Watch screen does the same by touch, in landscape for a wide desktop and with a
  pinch to zoom; `lpctl screen displays`, `start`, `stop` and `status` list, record and end
  shares (LP-148, LP-149, LP-150, LP-151, LP-173).
- Linux shares through the ScreenCast portal, or `wlr-screencopy` where the portal is missing,
  encoded by ffmpeg with VA-API where it works, with a Sharing notification whose action stops
  it; a Mac shares through ScreenCaptureKit and VideoToolbox once Ligature is allowed to
  record the screen; the phone shares after Android's dialog, or with nobody at it once the
  app is granted screen capture over `adb`, which its **Remote control** card reports
  (LP-149, LP-150, LP-151, LP-167).
- A share is light and quick: frames leave the sharer as soon as they are encoded, at 60 a
  second by default, a keyframe comes at the start and on request rather than every second, a
  still screen sends almost nothing, a Linux capture that stalls restarts itself, and a viewer
  takes from a sharer only a picture no larger or faster than it asked for, every frame
  bounded by that picture, dropping to the next keyframe when it falls behind, and a Linux
  share runs only while its notification shows, which comes back when dismissed and which no
  other device can close (LP-149, LP-150, LP-151, LP-176, LP-177, LP-186, LP-187).
- A share's picture format is agreed between the two devices: full colour detail (4:4:4),
  which keeps coloured text sharp, where both ends handle it, and 4:2:0 otherwise; `lpview
  --chroma` narrows the choice (ADR 0044, LP-162).
- `lpview --actual-size` shows a shared screen at its real size, adjusted for a high-DPI
  screen at either end and scrolling when larger than the window, since displays now report
  their scale (ADR 0045, LP-164).
- `lpctl permissions` lists which of Accessibility, Input Monitoring and Screen Recording the
  Mac's daemon holds, `lpctl permissions open <permission>` shows the page where each is
  turned on with Ligature listed, `lpctl permissions reset` clears them so macOS asks again,
  and a permission allowed takes effect without a restart (LP-169).
- The daemon on Linux touches no desktop portal until the session has passed its environment
  to systemd, and tries the InputCapture and ScreenCast portals again while they are missing
  (LP-157).
- Every release carries `SHA256SUMS` over its files and `SHA256SUMS.minisig`, its signature by
  the project's release key; the install guide has the key and the commands that verify a
  download (LP-139).
- `lpctl upgrade` installs the latest release on a desktop after verifying it, `lpctl upgrade
  --check` only reports, and every other `lpctl` command run on a terminal says once a day
  when a newer release exists (`LIGATURE_NO_UPDATE_CHECK=1` switches the check off) (LP-140).
- A user guide, one page per topic with each platform's limits named, and a documentation
  site carrying the guide and the specification (LP-136, LP-137, LP-181).

### Changed

- On macOS the daemon runs from its own application bundle, `~/Applications/Ligature.app`,
  and shows other devices' notifications through the user notification centre: the
  permission prompt names Ligature, icons are shown, a dismissal on another device clears
  the copy, and closing a copy on the Mac clears it everywhere; the Script Editor step is
  gone, and `ligatured` is no longer installed in `~/.local/bin` on macOS (LP-94).
- Every `lpctl` listing ends with the commands that act on what it shows, each with what it
  does, and `lpctl pair` on a device that already has its keys says how a new device is added
  (LP-170).
- `lpctl caps` names each device, this device's own capabilities first (LP-172).
- A refusal from another device reads as that device's refusal, by name and with its reason,
  rather than as a trust-chain error (LP-155).
- The phone's node never needs the app opened: it starts by itself after an update as at
  boot, a failed start is tried again until it succeeds, and its notification can be swiped
  away while it keeps running (LP-165).
- A revoked device loses its live sessions at once on every device of yours, not only at
  their next start, and a stream a device cannot take is reset with a code that says why
  (ADR 0038, ADR 0039, LP-124, LP-125).
- On an Apple ISO keyboard the key under Escape and the key beside the left Shift are read as
  their caps say, both ways (LP-160).

### Fixed

- A device of yours could no longer be cut off by another of your devices inventing a person:
  a revocation by anyone but you counts only from a person you know, and never over your own
  keys. Before, any paired device could sign a made-up person's role over a key of yours and
  that person's revocation of it, and every device honoured it, so a lost device could keep
  its own revocation from ever arriving (ADR 0047, LP-185).
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
