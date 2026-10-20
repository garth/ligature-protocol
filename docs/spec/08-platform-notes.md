# 08. Platform Notes

**Status:** Draft.

## 1. Intent

Each platform constrains what a device can offer and how it can run. This chapter records
those constraints so capability specs do not assume more than a platform allows.

## 2. Linux (Wayland)

The reference daemon (`ligatured`) runs as a systemd user service and exposes a control socket for
`lpctl` (D-Bus integration is a later milestone). Clipboard, notifications and remote input on
Wayland go through compositor protocols and portals (for example `wlr-data-control`, `xdg-desktop-
portal`), which differ by compositor. On a multi-account machine each account runs its own
`ligatured` as a systemd user service for user-scoped capabilities, and a system-level service owns
the device principal and device-scoped capabilities, learning who is present from logind. LP relies
on the OS's account isolation to keep one account's keys and clipboard from another. The account's
daemon does not depend on the session's environment: it finds the compositor socket in the runtime
directory and runs the Wayland side in a small helper process whose environment names that socket,
advertises the clipboard only while a compositor is reachable, and keeps running (gossip,
revocations) when none is. Open question: the minimum set of compositors and portals to support
(Hyprland is the first target), and how the system service drives audio and display without a
session of its own.

## 2b. macOS

A macOS machine is a full peer. The binding daemon runs as a per-user launchd agent and
the principal daemon as a launchd daemon, mirroring the Linux split; presence comes from
the console-user and session state that the system exposes to daemons. Bonjour is the
native mDNS/DNS-SD implementation and CoreBluetooth the BLE stack, so discovery needs no
extra components. What a third-party daemon may offer is bounded by platform permissions:

- **Clipboard**: NSPasteboard has no change notification, so the watcher polls the change
  count (every 250 ms in the reference daemon); reading the pasteboard from a background
  process is permitted.
- **Notifications**: a daemon may post notifications, but may not read other
  applications' notifications, so macOS can display forwarded notifications and cannot
  forward its own.
- **Remote input and displays**: injecting input needs the Accessibility permission and
  capturing or mirroring the screen needs Screen Recording; both are granted per
  application by the user under Transparency, Consent and Control.
- **Keys**: the Keychain stores the local store's keys; the Secure Enclave holds only
  P-256 keys, so an Ed25519 key is software-held and may be wrapped by one
  ([ADR 0027](../adr/0027-ed25519.md)).
- **Telephony and SMS**: not available to third parties; a macOS device offers none.
- **Distribution**: outside the App Store, with notarisation, since a sandboxed App Store
  build could not run the principal daemon.

Open question: whether the binding and principal daemons can share a signed application
bundle so the user grants permissions once.

## 3. Android

An Android phone or tablet is a full peer: the reference daemon runs as a library inside
an app, on the app's own threads, with its keys and store in the app's private storage.
The app supplies what the platform keeps from a library and nothing else; it speaks no
protocol of its own. LineageOS is the first target (no Google services, sideloaded;
[ADR 0033](../adr/0033-supported-platforms.md)). What the platform allows:

- **Running.** A foreground service of type `connectedDevice` with a persistent
  notification keeps the daemon alive and is restarted at boot; the user may still have
  to exempt the app from battery optimisation. mDNS needs the Wi-Fi multicast lock, which
  the service holds while it runs. The QUIC endpoint, discovery, gossip and manifests are
  the same code as on the desktops.
- **Clipboard.** Since Android 10 an app may read the clipboard only while its window has
  focus or it is the default keyboard; the change listener fires only when a read would
  be allowed. Writing is unrestricted. A phone therefore always receives (the daemon sets
  the primary clip from the background) and sends in one of two ways: automatically, when
  the app holds `READ_CLIPBOARD_IN_BACKGROUND`, a signature-level permission granted only
  to a build signed with the platform key (a self-built LineageOS); or on the user's
  request, from a quick-settings tile, a notification action, the app's own screen or the
  share sheet, each of which gives the app a moment of focus or hands it the text
  directly. The capability is advertised in both cases: what differs is when a `set` goes
  out, not whether the peer may send one.
- **Pairing.** The phone has a screen and a camera, so it shows its offer as a QR code
  and as text (the newcomer role); introducing another device from a pasted offer works
  too. A camera-based scanner is a later addition.
- **Device name.** The name under Settings → About phone, else the model, as the host
  name the device record carries ([ADR 0034](../adr/0034-keys-are-identifiers.md)).
- **Notifications, SMS, telephony.** Notification access, SMS and telephony need their own
  permissions and, for SMS, store policy approval; none is offered yet.
- **Keys.** The Android Keystore holds no Ed25519 keys on most devices (section 3a); the
  key file is software-held under the app's private directory.

Open question: whether a platform-signed build should be the documented LineageOS path
(an app in the ROM's `priv-app`) or stay a developer's option, and whether an input
method is ever worth offering as the automatic path on unmodified ROMs.

## 4. iOS

Third-party apps cannot run a general background daemon. LP's iPhone support relies on
BLE accessory profiles (ANCS for notifications, AMS for media, HFP/MAP/PBAP for telephony
and contacts via a Linux device acting as an accessory) and on EU Digital Markets Act
interoperability APIs where they exist. Open question: which features are realistic with
no app installed versus with a companion app.

## 3a. Local store protection

Hardware-backed keystores on some phones support only ECDSA P-256, not Ed25519
([ADR 0027](../adr/0027-ed25519.md)). On such devices the Ed25519 key is software-held and
may be wrapped by a hardware key; this is a platform constraint, not a protocol one.

Each device's local store (chapter 06) holds keys and trust state. Implementations are
encouraged to keep it under the account's data directory and use the secret service or a
TPM-backed key on Linux, the Keystore on Android and the Keychain on iOS. This is guidance:
device security is out of scope and conformance does not depend on it
([ADR 0024](../adr/0024-device-security-out-of-scope.md)).

## 4a. Networks and IPv6

Where both devices have global IPv6 addresses, a direct QUIC connection usually succeeds
without hole punching or a relay, since there is no NAT to traverse (only stateful
firewalls, which commonly admit outbound-initiated flows). Implementations should try IPv6
first when a discovery record offers it. IPv4 behind CGNAT, common on mobile networks, is
where relays are most often needed. Open question: whether to require IPv6 support in all
implementations.

## 5. Home hubs and headless devices

Hubs have no screen or camera and never rely on hardware for pairing: they serve a basic
web page on the LAN that shows the ceremony's fingerprint and code (chapter 02 section 5).
A hub is also the default personal relay and hole-punch coordinator (chapter 04), so
it should be always on and reachable from the internet, which on an IPv4-only home
connection means a port forward or a global IPv6 address. Before the first owner claims the device,
anyone on the LAN can open its pairing page and claim it; this setup-window risk is
accepted for milestone 1 ([ADR 0029](../adr/0029-ceremony-code-from-tls-exporter.md)). If
later judged insufficient, the options are a claim window opened by a power cycle or a
printed code.

## 6. Open questions

- Which platforms must be supported by the first public release?
- How are platform-specific capability limitations expressed in descriptors?
