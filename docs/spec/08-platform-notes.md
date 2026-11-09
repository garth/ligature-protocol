# 08. Platform Notes

**Status:** Draft.

## 1. Intent

Each platform constrains what a device can offer and how it can run. This chapter records
those constraints so capability specs do not assume more than a platform allows.

## 2. Linux (Wayland)

The reference daemon (`ligatured`) runs as a systemd user service, installed by the release
tarball's `install.sh` with its state in `$XDG_DATA_HOME/ligature` and its control socket in
`$XDG_RUNTIME_DIR/ligature`, and exposes that socket to
`lpctl` (D-Bus integration is a later milestone). The clipboard, input and screens on Wayland
go through compositor protocols and portals (`wlr-data-control`, `xdg-desktop-portal`), which
differ by compositor, and notifications through the session bus (below). On a multi-account
machine each account runs its own `ligatured` as a systemd user service for user-scoped
capabilities, and a system-level service, when it exists, will own the device principal and
device-scoped capabilities, learning who is present from logind (until then the account's
daemon offers them, [ADR 0040](../adr/0040-binding-stands-in-for-principal.md)). LP relies
on the OS's account isolation to keep one account's keys and clipboard from another. The account's
daemon does not depend on the session's environment: it finds the compositor socket in the runtime
directory and runs the Wayland side in a small helper process whose environment names that socket,
advertises the clipboard only while a compositor is reachable, and keeps running (gossip,
revocations) when none is. The helper reads a selection's text, HTML, PNG and file list
(`text/uri-list` of local `file:` URIs) and owns a data-control source for what peers
send: pushed parts from its copy, an announced part fetched from the peer only when the
compositor asks for that type, and a file list as `file:` URIs of files fetched into
`$XDG_RUNTIME_DIR/ligature/clipboard` when a file manager pastes (chapter 05 section 3b).
The QUIC endpoint takes a port the system assigns unless one is fixed (`--port`); the
service fixes 47471 so that behind a host firewall one rule lets peers dial in,
otherwise the machine connects only when it dials. Notifications both ways: the daemon
displays other devices' notifications by calling the freedesktop notification service on
the session bus, and sees what every application posts by being a monitor of that bus,
whatever notification daemon is in use, so Linux emits and displays (chapter 05 section
3c). Files (chapter 05 section 3d) land in the user's download folder, `XDG_DOWNLOAD_DIR`
from `user-dirs.dirs` or else `~/Downloads`, as a part file renamed into place once
verified; the desktop sends with `lpctl files send` and fetches with `lpctl files get`, and
a received file is announced by a local notification.

Remote input (chapter 05 section 3g) goes through the InputCapture portal, which Hyprland's
portal backend implements: the daemon sets a pointer barrier on each mapped edge of the
outputs' rectangles, and the compositor captures the keyboard and mouse the moment the
pointer crosses one, before the daemon decides anything, delivering them over libei until
the daemon releases them. So every barrier hit that does not become a drive gives the input
back at once, or the desktop is left without a keyboard and mouse. Hyprland's portal takes a
barrier only on a rectangle's boundary, which for a right or bottom edge is one past the
last pixel. Injection uses the wlroots virtual pointer and virtual keyboard protocols, in a
helper as the clipboard does. Hyprland takes a virtual keyboard's modifiers only from its
modifier reports, never from its key events, so the helper reports them after every
modifier key, and it applies a virtual keyboard's keymap when the keyboard is made, so
typed text goes through a short-lived keyboard whose keymap names each character. Keys are
read and found with xkbcommon against the seat's keymap and the locale's compose table
([ADR 0043](../adr/0043-keys-carry-their-meaning.md)). The portal must not be started before
the session has named its desktop and Wayland display: started earlier by the daemon, which
runs from the user's service manager, it settles on fallbacks for the whole login and offers
no InputCapture or ScreenCast, so the daemon waits for the session's environment before
touching it.

Displays (chapter 05 section 3h) are captured through the ScreenCast portal. Its own picker
chooses what is shared, so a viewer's choice of display is a request, and the restore token
it hands back is kept so the picker shows once. The PipeWire stream is read by GStreamer's
`pipewiresrc` and encoded by ffmpeg (VA-API where a test encode works, `libx264`
otherwise), child processes the daemon feeds and reads, so neither library is needed for
the rest of the daemon. `h264_vaapi` keeps two frames in flight by default (39 ms at 30 fps
here), so it runs with `-async_depth 1`; a raw H.264 stream marks where a frame begins but
not where it ends, so the daemon takes a whole-looking frame when the encoder pauses rather
than when the next one begins. A 4:4:4 share comes only from `libx264`, since VA-API encodes
4:2:0 alone ([ADR 0044](../adr/0044-screen-formats-are-negotiated.md)); the reference
viewers ask for 60 frames a second and a sharer agrees to at most 60, and `lpview` decodes
with ffmpeg's software decoder, asking for 4:4:4 first. Hyprland shows no indicator of a
screen cast, so the daemon posts a notification with a Stop action for the share's life, and
the share depends on it: one whose notification cannot be posted is refused, one whose
notification the user dismisses shows it again (a few times a minute at most), and a peer
cannot close it (chapter 09 section 4.4); a server that hides popups after a timeout, as
Omarchy's does whatever the request, keeps it in its list, where its Stop action still works.
`pipewiresrc` can stop delivering
while the portal logs "Out of buffers"; the daemon starts a capture that has gone quiet
again on the same portal session. Where the portal is missing, `wlr-screencopy` copies an
output instead.

A window is shared only as a cast, from the portal's picker opened on window sources, since
the portal offers no list a daemon could name a window from and `wlr-screencopy` copies
outputs alone; the pick is never remembered, and a window that changes shape is fitted into
the agreed picture with borders. A share's sound is what plays on the default output: its
monitor, read through PipeWire's PulseAudio interface and encoded as Opus by another ffmpeg,
which a machine without `libopus` or a default output simply does not offer. A cast offered
to the machine opens in `lpview`, which the daemon starts in the session from beside its own
executable, and `lpview` plays a share's sound through ffmpeg onto the default output.

Open question: the minimum set of compositors and portals to support (Hyprland is the first
target), and how the system service drives audio and display without a session of its own.

## 2b. macOS

A macOS machine is a full peer. The binding daemon runs as a per-user launchd agent
from an application bundle, `~/Applications/Ligature.app` (installed by the release
tarball's `install.sh`, state in `~/Library/Application Support/ligature`, control socket
in `$TMPDIR/ligature`, the UDP port fixed at 47471), because macOS grants notifications,
Accessibility, Input Monitoring and Screen Recording to a bundle by its identifier; the
principal daemon, when it exists, will be a launchd daemon, mirroring the Linux split, with
presence from
the console-user and session state that the system exposes to daemons. Bonjour is the
native mDNS/DNS-SD implementation and CoreBluetooth the BLE stack, so discovery needs no
extra components. What a third-party daemon may offer is bounded by what Apple lets a
program do on the owner's machine, and where the Mac does less than Linux below, that is
why; each such limit is recorded as the platform's choice and not worked around:

- **Clipboard**: NSPasteboard has no change notification, so the watcher polls the change
  count (every 250 ms in the reference daemon); reading the pasteboard from a background
  process is permitted. Types map through a table (`public.utf8-plain-text`,
  `public.html`, `public.rtf`, `public.png`, `public.tiff`, `public.file-url`); a value a
  peer sent is one pasteboard item with the text and pushed parts and a lazy data provider
  for each announced type, plus one item per promised file whose `public.file-url` is
  answered once the file is fetched into
  `~/Library/Caches/org.ligatureprotocol.ligatured/clipboard`, which Finder copies from.
- **Notifications**: a daemon may post notifications, but may not read other
  applications' notifications, so macOS can display forwarded notifications and cannot
  forward its own. The user notification centre serves only a process inside an
  application bundle, which is why the daemon runs from one: it asks for permission once,
  under the bundle's name, and advertises display only once the user allows it. A copy is
  posted under an identifier made of the origin's key and its id, so a repeated post
  replaces it and a dismissal from another device removes it; the icon travels as an
  image attachment; the user closing or clicking a copy is reported to the origin and the
  other devices. The same binary run outside the bundle displays nothing. A Focus mode
  hides the notifications as it hides any other.
- **Files**: received files land in `~/Downloads` the same way as on Linux, and `lpctl
  files` sends and fetches. The milestone 4 run (LP-87) wrote files there from the launchd
  agent with no Transparency, Consent and Control prompt recorded.
- **Remote input**: posting events and filtering them in an event tap needs the
  Accessibility permission, and seeing the keyboard needs Input Monitoring; both are
  granted to the bundle by the user under Privacy and Security, and macOS prompts for each
  once. The Mac accepts input while Accessibility is held and provides it while both are,
  so withdrawing either removes the family from every peer's view. macOS has no pointer
  barriers: the daemon watches the cursor in its tap and starts a drive when the cursor
  reaches a mapped edge. The taps see the events Ligature posts as they see the user's, so a
  Mac the phone's touchpad drives carries that pointer on across its edge, as Linux's
  InputCapture does; a drive that entered by an edge, or an edge leading back to the driving
  device, leads nowhere. With Terminal's Secure Keyboard Entry on, the keys still reached
  the driven device; other password fields are not yet tried. Every event of the session
  waits for a filtering tap, and a
  filtering tap whose process loses Accessibility holds back every click and key until it
  is gone, so the daemon watches with a listen-only tap and filters only while it drives
  another device. The permission checks a process can make at no cost answer from what it
  held at start, so a withdrawal is read through the live checks, and a grant of Input
  Monitoring takes effect only in a new process (System Settings offers to quit and reopen
  an application for it). While a permission was missing at start the daemon asks a
  short-lived copy of itself what a new process holds, and restarts when one has been
  granted, launchd starting it again. On the owner's Mac (an ad hoc signed bundle), turning
  Input Monitoring off and on turned Accessibility off with it. `CGRequestPostEventAccess`
  shows no alert; `AXIsProcessTrustedWithOptions` with the prompt option does, and a
  permission's page in System Settings lists the bundle only once it has asked.
- **Displays**: ScreenCaptureKit captures a display for a share, and serves only a process with
  the Screen Recording permission, which the user grants to the bundle once under Privacy and
  Security. macOS applies a newly granted permission to the next start of the process, so the
  first share is refused, and the daemon restarts by itself once it sees the permission allowed,
  as for input. While a share runs the menu bar shows the system's own sharing indicator, and
  stopping the share there ends it with `consent`. ScreenCaptureKit delivers a frame only when the
  display changed, so a still screen's frame is repeated once a second. A launchd agent of process
  type `Background` is throttled so as not to disturb the user, which halved the frame rate and
  slowed input to a quarter of a second, so the agent is `Interactive`. ScreenCaptureKit lists
  the windows on screen, so a Mac offers them by id and shares one through its own filter,
  scaled to fit the agreed picture. The system's sound comes from the same stream on macOS 13
  and later, which is when Apple added it to the capture; on an older macOS a share is silent,
  since Apple gives a program no other way to what the machine plays. A cast offered to the
  Mac opens in `lpview`, started by the agent, which plays sound through AudioToolbox.
- **Keys**: the Keychain stores the local store's keys; the Secure Enclave holds only
  P-256 keys, so an Ed25519 key is software-held and may be wrapped by one
  ([ADR 0027](../adr/0027-ed25519.md)).
- **Telephony and SMS**: not available to third parties; a macOS device offers none.
- **Distribution**: outside the App Store, with notarisation, since a sandboxed App Store
  build could not run the principal daemon.

The principal daemon, when it exists, runs from the same bundle, so the user grants each
permission once.

## 3. Android

An Android phone or tablet is a full peer: the reference daemon runs as a library inside
an app, on the app's own threads, with its keys and store in the app's private storage.
The app supplies what the platform keeps from a library and nothing else; it speaks no
protocol of its own. LineageOS is the first target (no Google services, sideloaded;
[ADR 0033](../adr/0033-supported-platforms.md)). What the platform allows:

- **Running.** A foreground service of type `connectedDevice` keeps the daemon alive, with
  nothing depending on the app's screen: it starts at boot and after an update of the app,
  which kills it, and a start that fails is tried again until it succeeds. Android requires
  a notification for such a service and raises its channel to low importance at the least,
  so it sits in the silent group; the user may swipe it away and the service keeps running,
  until its next start posts it again. The user may still have to exempt the app from
  battery optimisation. mDNS needs the Wi-Fi multicast lock, which
  the service holds while it runs, and a responder started before Wi-Fi has an address
  hears nothing, so the node starts on the first available network and restarts discovery
  on every change of network. The QUIC endpoint, discovery, gossip and manifests are the
  same code as on the desktops.
- **Clipboard.** Since Android 10 an app may read the clipboard only while its window has
  focus or it is the default keyboard; the change listener fires only when a read would
  be allowed. Writing is unrestricted. A phone therefore always receives (the daemon sets
  the primary clip from the background) and sends in one of three ways. First, automatically, when
  the app holds `READ_CLIPBOARD_IN_BACKGROUND`, a signature-level permission granted only
  to a build signed with the platform key, which means a ROM built from source, since the
  LineageOS project's own builds are signed with keys it keeps. Second, automatically, while
  the user has the accessibility service on, which sees SystemUI's clipboard indicator
  appear (the listener does not fire for an unfocused app, as LineageOS 24 on Android 17
  confirmed), adds
  an invisible window to take the focus for a moment, reads and sends. Android shows its own
  toast for the first read of each clip, unless the user has turned off Show clipboard access
  (`clipboard_show_access_notifications`, a secure setting no app may read, so the app always
  points to it). A second copy while the indicator is still up, about
  six seconds, makes no new event, so the service reads again once it would have gone and
  keeps doing so while each read finds a new clip, by the timestamp Android puts on it; a
  clip the copying app marked sensitive is passed over. The service tells the indicator apart
  from the notification shade and quick settings by its event, which carries no text or one
  item where theirs carry many, and whose closing is a pane disappearing. Third, on the
  user's request,
  from a quick-settings tile, a notification action, the app's own screen or the share
  sheet, each of which gives the app a moment of focus or hands it the text directly. The
  capability is advertised in all three cases: what differs is when a `set` goes out, not
  whether the peer may send one. The clip carries text and HTML itself; an image or a file
  selection a peer sent is a content URI of the app's own provider, fetched from the peer
  only when an application opens it, into the app's cache directory. Since few Android
  applications paste files, a file list also raises a notification whose **Save** lands
  the files in Downloads, and **Receive now** on the app's screen asks the other devices
  for their clipboard. What the app reads when it may includes the clip's HTML and content
  URIs, an image up to 64 MiB becoming a part and anything else a file of the selection.
  The system's copy overlay asks the provider for a thumbnail the moment a clip is set;
  it gets a placeholder, so only an application's read fetches.
- **Pairing.** The phone has a screen and a camera, so it shows its offer as a QR code
  and as text (as the newcomer); introducing another device from a pasted offer works
  too. A camera-based scanner is a later addition.
- **Device name.** The name under Settings → About phone, else the model, as the host
  name the device record carries ([ADR 0034](../adr/0034-keys-are-identifiers.md)).
- **Notifications.** A notification listener service, which the user enables under
  notification access, sees what every app posts and removes and can cancel one, so the
  phone emits and displays (chapter 05 section 3c); the capability's `emits` follows the
  grant. Ongoing, silent and summary notifications stay on the phone, and the user can
  switch any app off.
- **Files.** The app has no download folder of its own: a received file is a row in
  MediaStore's Downloads collection, inserted pending, written by the node through a
  descriptor the app opened, and published once verified, so the Files app shows it under
  Downloads; the app gives a clashing name a numbered suffix before inserting the row.
  Sending starts from the share sheet: the user shares one or several files to Ligature and
  picks a device that receives them; the app opens each one and the node reads it through
  the descriptor. Each transfer is a
  notification with a progress bar and a cancel. The phone takes one file at a time
  (`maxStreams` 1) and sends up to four at once (chapter 05 section 3d).
- **Remote input.** The phone drives the desktops: a touchpad screen turns gestures into
  pointer motion, clicks and scrolling, and a text field types through the phone's own
  keyboard and input method, which gives characters rather than keys. It is driven only
  through an accessibility service, which the user turns on in Settings and which is the one
  way Android lets an app inject, so it accepts input only while that is on, and the
  descriptor follows. A touch screen has no pointer or buttons: the app draws a pointer, the
  left button is a finger at it, the right is Back and the middle is Home, and scrolling is a
  swipe. Characters and keys go through the service's input connection, which Android gives
  an accessibility service only from Android 13, so they reach a focused text field and
  nothing else, and a held key does not repeat, since only its press and its release reach
  the phone and the connection repeats nothing. A person's
  own touch cancels a finger the service holds down, and so does SystemUI as the keyguard's
  PIN pad rises in the middle of a swipe, so a stroke Android cancels while the remote button
  is held starts again on the next move, or the bouncer stays half open and the next tap
  closes it. A continued stroke of no length is never finished, so a move that goes nowhere
  (a Mac trackpad reports one before a release) is not made. The screen is kept on for the
  drive.
- **Displays.** Only an app may capture the screen, after the user consents in the media
  projection dialog for every share; the dialog shares one app unless the user picks the
  entire screen. An app in the background may not open the dialog, so a viewer's request
  becomes a notification whose tap opens it, unless the user has the accessibility service
  on, which lifts that limit: then the dialog opens at once and the screen wakes for it. The
  service never answers the dialog. On a locked phone the dialog first sends the keyguard to
  its PIN, pattern or password, a secure window a viewer cannot see. A wake lock lights the
  screen from the background only with `TURN_SCREEN_ON` on Android 14 and later (Android 17
  still lets it through without, and logs it). The `PROJECT_MEDIA` app op, which only
  `adb shell appops set` changes, makes the dialog answer itself: on Android 17 a request
  then shares within about half a second, even on a locked phone, whose lock screen is shared
  but whose PIN, pattern or password screen is not. The capture runs in a foreground service of
  the media projection type while Android shows its cast indicator, and `MediaCodec`
  encodes it; stopping the cast from the system ends the share with `consent`. A shared
  desktop is decoded by `MediaCodec` into a surface on the app's watch screen, where touch
  drives it. A cast starts from the app's Cast card, so the dialog opens where the person
  is; the dialog, not the app, decides between one app and the whole screen. What the phone
  plays is captured under the projection only with the recording permission, the same one as
  the microphone's, which the app asks for from that card alone; Android leaves out calls
  and any app that opts out of capture, so a cast may be silent where sound was expected,
  and that is the platform's rule. `MediaCodec` encodes and decodes the Opus. A cast offered
  to the phone opens the watch screen while the app is in front or the accessibility service
  is on, and is otherwise a notification, since Android keeps a background app from opening
  itself.
- **SMS, telephony.** SMS and telephony need their own permissions and, for SMS, store
  policy approval; neither is offered yet.
- **Keys.** The Android Keystore holds no Ed25519 keys on most devices (section 4a); the
  key file is software-held under the app's private directory.

## 4. iOS and iPadOS

Apple's phones and tablets are a closed platform: a third-party app runs only while the
user has it open or for the few background modes Apple grants, may not read other
applications' notifications or, without a prompt, their clipboard, and gets no daemon of any
kind. An iPhone or iPad can therefore never be the always-on peer that Android is. That is
Apple's choice about what the owner of the device may run on it, not a limit of the
protocol, and the project exists in part as an open alternative to it: capabilities are left
out here only because the platform is closed, never because they would not fit. This
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
  open, which is when they are wanted; an iPad could be a `screen` viewer for a desktop this way.
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

## 4a. Local store protection

Hardware-backed keystores on some phones support only ECDSA P-256, not Ed25519
([ADR 0027](../adr/0027-ed25519.md)). On such devices the Ed25519 key is software-held and
may be wrapped by a hardware key; this is a platform constraint, not a protocol one.

Each device's local store (chapter 06) holds keys and trust state. Implementations are
encouraged to keep it under the account's data directory and use the secret service or a
TPM-backed key on Linux, the Keystore on Android and the Keychain on iOS. This is guidance:
device security is out of scope and conformance does not depend on it
([ADR 0024](../adr/0024-device-security-out-of-scope.md)).

## 4b. Networks and IPv6

Where both devices have global IPv6 addresses, a direct QUIC connection usually succeeds
without hole punching or a relay, since there is no NAT to traverse (only stateful
firewalls, which commonly admit outbound-initiated flows). Implementations should try IPv6
first when a discovery record offers it. IPv4 behind CGNAT, common on mobile networks, is
where relays are most often needed. Open question: whether to require IPv6 support in all
implementations.

## 5. Home hubs and headless devices

Hubs have no screen or camera and never rely on hardware for pairing: they serve a basic
web page on the LAN that shows the ceremony's offer (chapter 02 section 5).
A hub is also the default personal relay and hole-punch coordinator (chapter 04), so
it should be always on and reachable from the internet, which on an IPv4-only home
connection means a port forward or a global IPv6 address. Before the first owner claims the device,
anyone on the LAN can open its pairing page and claim it; this setup-window risk is
accepted ([ADR 0029](../adr/0029-ceremony-code-from-tls-exporter.md)). If
later judged insufficient, the options are a claim window opened by a power cycle or a
printed code.

## 6. Open questions

- How are platform-specific capability limitations expressed in descriptors?
