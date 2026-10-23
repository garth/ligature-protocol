---
name: android-app
description: USE when changing the Android app under impl/android or the ligature-android crate it wraps. Contains the app's structure, what Android allows, the Compose conventions and how the phone is driven over adb.
---

# The Android app

The daemon runs on the phone as a library (`impl/crates/ligature-android`, a `cdylib` behind
UniFFI 0.32 proc macros) inside a Kotlin app (`impl/android`, Compose, Material 3, min SDK
29). The app supplies what Android keeps from a library and nothing more; protocol logic
stays in Rust.

## Structure

| Kotlin                                                               | Role                                                                                                                            |
| -------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `LigatureApp`, `NodeHolder`                                          | Process-wide node, created once, configured with the data and cache directories                                                 |
| `NodeService`                                                        | Foreground service of type `connectedDevice`, holds the multicast lock, started at boot (`BootReceiver`) and when the app opens |
| `MainActivity`, `ui/MainScreen`, `ui/QrCode`                         | The one screen: status, devices, the pairing offer as a QR code, Send and Receive now, Save                                     |
| `SendClipboardActivity`, `SendClipboardTileService`, `ShareActivity` | The moments the app may read the clipboard: a focused activity, the quick-settings tile, the share sheet                        |
| `ClipboardProvider`                                                  | The `ContentProvider` serving announced parts and files; a thumbnail request gets a placeholder, a real read fetches            |
| `NotificationListener`                                               | The listener that emits the phone's notifications and will serve media sessions                                                 |
| `Transfers`                                                          | One progress notification per transfer with Cancel, then Sent or Received                                                       |

The Rust side (`ligature-android/src`): `node.rs` (the `Node` the Kotlin calls, a Tokio
runtime, `with_runtime`), `clipboard.rs` (`HostClipboard`), `notifications.rs`,
`files.rs` (MediaStore pending rows written through `/proc/self/fd`), and the `Host` trait
the app implements. The Kotlin bindings are generated into `app/build/generated/uniffi/`
and never committed; a change to an exported type or function regenerates them on the next
Gradle build.

## What Android allows, and the app's answers

- The clipboard is readable only by a focused app or the keyboard. Receiving is automatic;
  sending is on a tap (tile, notification action, the screen, the share sheet), and
  automatic only with `READ_CLIPBOARD_IN_BACKGROUND` on a platform-signed build, which an
  official LineageOS build cannot be. Do not try to read it from the service.
- Files land in MediaStore's Downloads as pending rows, finished after the hash check;
  names clash by MediaStore's numbering.
- Notifications need `POST_NOTIFICATIONS` on 13 and later; the foreground service needs its
  type declared; battery optimisation must be off for the node to live in the background.
- New permissions are opt-in from the app's own settings (milestone 6 adds SMS, calls and
  contacts that way): never requested at first run, and the capability's descriptor reflects
  what is held so the manifest changes live.
- A content URI served by `ClipboardProvider` is the way another app pastes an image or a
  file; `openFile` fetches on first open and must never fetch for a thumbnail.

## Compose conventions

Material 3 components, one screen in `ui/`, state hoisted from the node through the
service, `safeDrawingPadding` on the root, strings inline for now (no `strings.xml` beyond
the app name until the app has a second language). Keep the screen plain; the app is a host
for the node, not a product surface yet.

## Build and run

```sh
cd impl/android
./gradlew assembleDebug          # cargo ndk for arm64-v8a and x86_64, uniffi-bindgen, the APK
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell monkey -p org.ligatureprotocol.android -c android.intent.category.LAUNCHER 1
```

`adb` is `~/Android/Sdk/platform-tools/adb`. A reinstall keeps the phone's identity. The
release APK is signed when `LIGATURE_ANDROID_KEYSTORE` and its password, alias and key
password are set (the release workflow does), and unsigned otherwise.

## Driving the phone from the tool shell

- Compose screens: `adb shell uiautomator dump` and `adb shell input tap x y`; the
  notification shade is not in that dump, so `adb exec-out screencap -p`, read the PNG, and
  tap by coordinates. Transfer and clipboard notifications sit in the collapsed Silent
  group: expand the group, then the notification, before its action shows.
- Put text on the clipboard and send it:
  `adb shell "am start -a android.intent.action.SEND -t text/plain --es android.intent.extra.TEXT 'words' -n org.ligatureprotocol.android/.ShareActivity"`.
- `adb logcat -s ligature` for the node's log; `debug_logging` in the node config raises it.
- If adb says `unauthorized`, `adb kill-server` makes the phone prompt again. The owner's
  phone is a Pixel 8 Pro on LineageOS and enumerates on one USB port only; see the device
  notes in memory.

## Tests

`cargo test -p ligature-android --test two_nodes` runs the node end to end with a
`FakeHost` on real sockets; it is the regression test for anything in `node.rs`. The
Kotlin has no unit tests yet; a change to the app is checked on the phone and recorded on
the ticket.
