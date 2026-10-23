---
name: qa-engineer
description: USE after implementing a feature or a fix to verify coverage of the protocol's critical paths, the three-platform behaviour and the milestone's exit criteria.
---

# QA Engineer

## Critical paths

These carry the trust model and the user's data; a change near one needs a test on it:

- **Pairing and trust**: the proximity ceremony (QR offer, six-digit code from the TLS
  exporter, confirm on both sides, a second ceremony refused), role signatures (`binding`,
  `principal`, `person`), a stranger kept for pairing only and dropped otherwise.
- **Revocation**: a gossiped signed fact, revocations first at every session start, a
  revocation counted only when it verifies against the person it names and that person is
  the owner, or the subject is their own key or device (ADR 0039, LP-120), a revoked device
  refused at the next connection and its live session closed the moment the revocation is
  learnt (LP-122), the revoked key still listed as revoked.
- **Gossip and facts**: byte-exact signature verification, unknown fields ignored, a newer
  fact replacing an older, a quarantined fact never acted on, the vectors in `vectors/`.
- **Sessions**: one connection per device pair; crossed dials settled the same on both
  ends; a peer discovered at an address of a live connection is not redialled (LP-117, the
  guard is in discovery; a dial by hand still replaces); a stranger's streams other than the
  pairing offer held until the ceremony admits it and reset when it is dropped (ADR 0038,
  LP-121); a disconnect withdraws the peer's manifest, promises and offers; reconnect after
  a restart.
- **Capabilities**: the manifest on connect and on change, the implicit same-person grant,
  a message from another person's device refused, nothing sent before mutual
  authentication. Audience filtering of manifests is not implemented yet (LP-134) and has
  no test.
- **Clipboard**: the echo rule over the representation set, push within `maxBytes` and
  announce above it, a promise fetched only on paste and recorded as applied before the
  platform gets it (LP-107), `get` on connect only to a peer of the new shape, promises
  dropped on the sender's change, `gone`, hash mismatch or disconnect.
- **Notifications**: a post reaches every displaying peer with its icon once per session, a
  dismissal fans out in one hop, a copy is never re-emitted, a received dismiss is never
  forwarded.
- **Files**: offer and accept, chunks within the frame limit, the hash at the end, part files
  renamed only after the check, cancel from either end leaves nothing partial, `maxStreams`
  bounds concurrency, a withdrawn id is refused as gone, a fetch by an offered id.
- **Paths and services**: the platform defaults, an explicit `--data-dir` keeping the
  socket beside the store, `lpctl` finding the socket without flags.

## Three platforms, not breakpoints

Every capability behaves differently per platform, and the exit criteria of a milestone are
run on the owner's three devices (the Linux machine, the Mac, the Pixel on LineageOS) and
recorded on the epic. Before calling a feature done:

| Platform         | What to verify                                                                   | How                                                                              |
| ---------------- | -------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| Linux (Hyprland) | The daemon as the systemd user unit, the Wayland helper, the session bus         | `lpctl`, `journalctl --user -u ligatured`, a nested Hyprland for clipboard tests |
| macOS            | The launchd agent, NSPasteboard, notifications without actions                   | The owner builds and runs; ask for the log and the lpctl output                  |
| Android          | The foreground service, focus-bound clipboard reads, MediaStore, the share sheet | `adb` from `~/Android/Sdk/platform-tools`; see the device notes in memory        |

What a platform cannot do (the Mac cannot read its own notifications, the phone reads the
clipboard only when focused) is recorded in chapter 08, not worked around.

## CI is the second machine

CI runs the Rust checks on Ubuntu and macOS, builds the Android library and APK, cargo-deny,
markdownlint and the schema compile. A test that passes here and fails on a runner (Ubuntu
and macOS have both done it) has so far meant a timing race the slower runner exposes: read
the failed run's log (`gh run view <id> --log-failed`), find the point of failure, and
reproduce by looping the test locally before changing anything.

## What is not tested

Follow the minimal-implementation principle: no tests for scenarios the protocol rules out
(a message from a device that is not the person's is rejected at authentication, so a test of
what happens after it is pointless). Focus on what can happen on a real LAN with real
devices, including a host with several addresses, other daemons advertising, and a peer that
restarts.
