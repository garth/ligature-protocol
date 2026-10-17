# 08. Platform Notes

**Status:** Draft.

## 1. Intent

Each platform constrains what a device can offer and how it can run. This chapter records
those constraints so capability specs do not assume more than a platform allows.

## 2. Linux (Wayland)

The reference daemon (`ligatured`) runs as a systemd user service and exposes D-Bus
interfaces. Clipboard, notifications and remote input on Wayland go through compositor
protocols and portals (for example `wlr-data-control`, `xdg-desktop-portal`), which differ
by compositor. On a multi-account machine each account runs its own `ligatured` as a
systemd user service for user-scoped capabilities, and a system-level service owns the
device principal and device-scoped capabilities, learning who is present from logind. LP
relies on the OS's account isolation to keep one account's keys and clipboard from another.
Open question: the minimum set of compositors and portals to support (Hyprland is the first
target), and how the system service drives audio and display without a session of its own.

## 3. Android

Background execution, battery optimisation and the clipboard restrictions introduced in
Android 10 shape what an LP app can do. Notification access, SMS and telephony require
specific permissions and, for SMS, Play Store policy approval. Open question: whether the
Android reference app targets Play Store distribution or F-Droid first.

## 4. iOS

Third-party apps cannot run a general background daemon. LP's iPhone support relies on
BLE accessory profiles (ANCS for notifications, AMS for media, HFP/MAP/PBAP for telephony
and contacts via a Linux device acting as an accessory) and on EU Digital Markets Act
interoperability APIs where they exist. Open question: which features are realistic with
no app installed versus with a companion app.

## 3a. Local store protection

Each device's local store (chapter 06) holds keys and trust state. On Linux it should live
under the account's data directory and use the secret service or a TPM-backed key where
available; on Android, the Keystore; on iOS, the Keychain. Open question: the minimum
protection an implementation must provide to call itself conformant.

## 4a. Networks and IPv6

Where both devices have global IPv6 addresses, a direct QUIC connection usually succeeds
without hole punching or a relay, since there is no NAT to traverse (only stateful
firewalls, which commonly admit outbound-initiated flows). Implementations should try IPv6
first when a discovery record offers it. IPv4 behind CGNAT, common on mobile networks, is
where relays are most often needed. Open question: whether to require IPv6 support in all
implementations.

## 5. Home hubs and headless devices

Hubs have no screen or camera, so pairing and recovery flows need an alternative to QR
codes. A hub is also the default personal relay and hole-punch coordinator (chapter 04), so
it should be always on and reachable from the internet, which on an IPv4-only home
connection means a port forward or a global IPv6 address. Open question: whether a trusted
device can pair a hub on its behalf.

## 6. Open questions

- Which platforms must be supported by the first public release?
- How are platform-specific capability limitations expressed in descriptors?
