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
revocations) when none is. The QUIC endpoint takes a port the system assigns unless one is
fixed (`--port`); behind a host firewall the fixed port and one rule let peers dial in,
otherwise the machine connects only when it dials. Open question: the minimum set of
compositors and portals to support
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
  the service holds while it runs, and a responder started before Wi-Fi has an address
  hears nothing, so the node starts on the first available network and restarts discovery
  on every change of network. The QUIC endpoint, discovery, gossip and manifests are the
  same code as on the desktops.
- **Clipboard.** Since Android 10 an app may read the clipboard only while its window has
  focus or it is the default keyboard; the change listener fires only when a read would
  be allowed. Writing is unrestricted. A phone therefore always receives (the daemon sets
  the primary clip from the background) and sends in one of two ways: automatically, when
  the app holds `READ_CLIPBOARD_IN_BACKGROUND`, a signature-level permission granted only
  to a build signed with the platform key, which means a ROM built from source, since the
  LineageOS project's own builds are signed with keys it keeps; or on the user's request,
  from a quick-settings tile, a notification action, the app's own screen or the share
  sheet, each of which gives the app a moment of focus or hands it the text directly. The
  capability is advertised in both cases: what differs is when a `set` goes out, not
  whether the peer may send one.
- **Pairing.** The phone has a screen and a camera, so it shows its offer as a QR code
  and as text (the newcomer role); introducing another device from a pasted offer works
  too. A camera-based scanner is a later addition.
- **Device name.** The name under Settings → About phone, else the model, as the host
  name the device record carries ([ADR 0034](../adr/0034-keys-are-identifiers.md)).
- **Notifications, SMS, telephony.** Notification access, SMS and telephony need their own
  permissions and, for SMS, store policy approval; none is offered yet.
- **Keys.** The Android Keystore holds no Ed25519 keys on most devices (section 3a); the
  key file is software-held under the app's private directory.

Open question: how an unmodified ROM gets the automatic path, since a platform-signed
build needs a self-built ROM: an app allowed to display over other apps may start its send
activity from the clip listener, which fires in the background on LineageOS 24; a
notification on every copy is the tap-once fallback; an input method is the last resort.

## 4. iOS and iPadOS

Apple's phones and tablets are a closed platform: a third-party app runs only while the
user has it open or for the few background modes Apple grants, may not read other
applications' notifications or, without a prompt, their clipboard, and gets no daemon of any
kind. An iPhone or iPad can therefore never be the always-on peer that Android is. This
section records what is possible, in three tiers, so a later milestone can start from it.
**No milestone is planned for Apple phones and tablets**: they stay last in
[ADR 0033](../adr/0033-supported-platforms.md) and are off the roadmap until the project
chooses to take them up. iPadOS has the same restrictions as iOS, and an iPad is a group
device like any tablet (chapter 10).

**No app on the device.** Two paths exist today, neither of which gives the device an LP
identity: the phone is identified by Bluetooth pairing or by Apple's own account, not by
the ceremony.

- **BLE accessory profiles to a Linux device or hub**, a bridge under
  [ADR 0010](../adr/0010-interop-bridges.md) that advertises the phone's capabilities on
  its behalf. ANCS gives the phone's notifications with dismiss and the positive and
  negative actions (most of `notifications` emit), AMS gives now-playing and transport
  control (`media`), and HFP, MAP and PBAP give call audio, SMS and contacts (`telephony`),
  with BlueZ, oFono and obexd in the accessory role. A Mac cannot fill the accessory role
  for most of these profiles.
- **A Mac as a Continuity bridge.** A Mac binding daemon that sets its pasteboard makes the
  text available to the person's iPhone through Universal Clipboard, and text copied on
  the iPhone reaches the Mac's daemon when something is pasted on the Mac. Text only, and
  whether the Mac's daemon sees the phone's copy before a paste or only after is untested.

**A companion app, in the foreground.** The reference library builds for iOS as it does
for Android, with Swift bindings from UniFFI, and an app may use QUIC over UDP, Bonjour
and the local network (after the local-network permission), so the app runs the whole
protocol: the same identity, ceremony, gossip, manifests and streams. What the platform
bounds:

- **Clipboard.** Writing is unrestricted. Reading what another application copied shows
  the system paste prompt (since iOS 16) unless the user presses the system paste control,
  so sending is on the user's request, as on Android: the app's screen, a share-sheet
  extension, or a Shortcuts action that the Action Button or Back Tap can trigger.
- **Notifications.** The app displays other devices' notifications as local
  notifications; it can never read other applications', so emit stays with ANCS above.
- **Files.** Sent from the share sheet, received into the app's folder in the Files app.
  A transfer gets about thirty seconds once the app leaves the foreground.
- **Remote input, camera, sensors, display.** Usable while the device is held and the app
  open, which is when they are wanted; an iPad can be a `display` for a desktop this way.
- **Keys.** The Keychain stores the local store's keys; the Secure Enclave holds only P-256
  keys, as on macOS (section 2b).

**The companion app in the background.** A suspended app has no QUIC connection, so the
device drops out of the device set until the user opens the app again. Two things survive
suspension: the Bluetooth background modes, which let the app keep scanning for LP's
service and be relaunched when a known peer appears, enough for clipboard text and
notification display over a low-rate channel but not for files or images, and which would
need the BLE transport specified (chapter 04); and scheduled background tasks, good for a
reconnect and gossip sweep a few times a day, not for live sync.

**EU interoperability.** Under the Digital Markets Act Apple has published a schedule of
interoperability features for the EU: notification forwarding to third-party devices,
proximity pairing, automatic audio switching and background execution for accessory apps
in iOS 26, with peer-to-peer Wi-Fi, close-range file transfer and media casting due in the
following release. If they ship as described, a hub gets notifications without ANCS and a
companion app gets the background execution the previous tier lacks, in that region only.
Timing and scope are to be verified when a milestone is planned.

A realistic first Apple milestone would be the first two tiers: a Linux hub bridging
notifications, media and telephony from the phone, and a Swift app over the shared library
for pairing, clipboard receive, clipboard send on request, file send from the share sheet,
and the device as a controller, camera or display while open.

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
