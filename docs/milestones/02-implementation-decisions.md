# Milestone 2: implementation decisions

Choices that are not part of the protocol but that the Android app needs settled. Each
entry names its ticket. Nothing here changes what goes on the wire; the phone speaks
exactly what the desktops speak.

## LP-77: the daemon is a library, bound with UniFFI

**Decision.** `crates/ligature-android` builds `ligatured`'s library side as a `cdylib`
for Android and exposes a small API to Kotlin through [UniFFI](https://mozilla.github.io/uniffi-rs)
0.32 proc macros: a `Node` (start, status, init, devices, revoke, clipboard, the two
ceremony roles, stop), a `Pairing` object (offer, confirm, abort), a `Host` trait the app
implements (set the clipboard, receive an event, receive a log line) and an `Event` enum.
The node owns a tokio runtime and runs the same loops `ligatured::run` runs, without the
control socket (LP-76). Hand-written JNI was passed over: far more `unsafe`, and a second
description of every type.

**Licence.** UniFFI is MPL-2.0, a file-level copyleft that never touches our files;
`deny.toml` allows it. The binding generator pulls in an unmaintained `basic-toml`
(RUSTSEC-2026-0374); it runs only on the host under the crate's `bindgen` feature and the
advisory is ignored with that note.

**`unsafe_code`.** The workspace forbids it. The exported C ABI needs `#[unsafe(no_mangle)]`,
which that lint covers, so this one crate sets its own lints with `unsafe_code = "allow"`;
no `unsafe` block is written in it.

**Clipboard direction.** The desktops' daemons watch the system clipboard themselves. On
Android only the app can read it, and only sometimes (LP-78), so the direction is
reversed: the app calls `clipboard_changed(text)` when it could read, and `read_text` on
the node's `Clipboard` answers with the last text set or reported, which is what the
synchroniser needs to avoid re-setting what is already there. Setting goes through the
host, which returns whether Android accepted it.

**Logging.** A `tracing` layer formats each event and hands it to the host's `log`, which
writes to logcat. No native log crate, no `unsafe`. The host slot is global and replaced
on each start, since Android keeps the process across service restarts.

**Threads.** Host methods are called from the node's tokio threads; every Kotlin
implementation must be safe to call from any thread. `setPrimaryClip` is.

## LP-78: the app

**Clipboard on Android 10 and later.** An app may read the clipboard only while it has
window focus or is the default keyboard; `OnPrimaryClipChangedListener` fires only when a
read would be allowed. Writing is unrestricted. The app therefore:

- **receives** by setting the primary clip from the foreground service: always works;
- **sends automatically** only when it holds `android.permission.READ_CLIPBOARD_IN_BACKGROUND`,
  a `signature` permission the manifest declares and that Android grants only to an APK
  signed with the platform certificate. A self-built LineageOS has that key; the README
  gives the `apksigner` command. The service registers the clip listener unconditionally
  and it simply does not fire without the permission;
- **sends on request** otherwise, through a transparent activity
  (`SendClipboardActivity`) that reads the clip the moment its window has focus and
  finishes, reached from a quick-settings tile, the notification's action and the main
  screen; and through a share-sheet target (`ShareActivity`) that receives the text in
  the intent and needs no clipboard read at all. Opening the app also sends what is on
  the clipboard.

An input method (a keyboard that may read in the background) was passed over as too
intrusive; an accessibility service cannot read the clipboard; root is not something the
app may assume.

**Service.** `NodeService` is a foreground service of type `connectedDevice` (declared
from API 29; required from API 34, where `CHANGE_WIFI_MULTICAST_STATE` is its
prerequisite). It holds the `WifiManager.MulticastLock` so the driver delivers mDNS,
starts the node and stops it on destroy. `BootReceiver` starts it at boot; the main
activity, the tile and the share target start it if it is not running. The user is asked
to allow unrestricted battery use.

**State and locations.** The node's data directory is `files/ligature/` in the app's
private storage, so `keys.cbor` and `store.sqlite` are reachable by this app alone; the
daemon's own 0700/0600 modes apply unchanged. The device name is `Settings.Global.DEVICE_NAME`,
else `Build.MODEL`. Logs are logcat, tag `ligature`.

**Toolchain.** Gradle 8.14 (wrapper committed), Android Gradle plugin 8.13, Kotlin 2.4 with
the Compose compiler plugin, Jetpack Compose with Material 3, minimum SDK 29 (Android 10,
where the clipboard rules began; LineageOS 17 and later), compile SDK 36, target SDK 35.
JNA (AAR) carries the UniFFI calls; ZXing core draws the QR code with no Google services.
Two `Exec` tasks run before `preBuild`: `cargo ndk` for `arm64-v8a` and `x86_64` (release
profile, into `app/build/rustJniLibs`) and `uniffi-bindgen` from the arm64 library (it
reads the metadata from the ELF file) into `app/build/generated/uniffi`. Neither output is
committed. `mise.toml` in `impl/android` names the JDK and Rust; the SDK is found through
`local.properties`.

**Signing.** Debug builds use the debug key. Release builds are unsigned; the README signs
them with `apksigner`, with a debug key for an ordinary sideload or the platform key for
automatic clipboard sending. No keystore is in the repository.

**Not minified.** R8 is off for now; JNA and the generated bindings rely on reflection and
the rules in `proguard-rules.pro` are kept ready for when it is turned on.
