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

| Kotlin                                              | Role                                                                                                                                                                                                         |
| --------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `LigatureApp`                                       | The `Application`; registers nothing but the process                                                                                                                                                         |
| `NodeHolder`                                        | The process-wide node, created once on first use with the data and cache directories, and the app's state: the clipboard reads, the files notification with its Save action, the host callbacks              |
| `NodeService`                                       | Foreground service of type `connectedDevice`, holds the multicast lock, started at boot (`BootReceiver`) and when the app opens; reads the clipboard itself only when `READ_CLIPBOARD_IN_BACKGROUND` is held |
| `MainActivity`, `ui/MainScreen`, `ui/QrCode`        | The one screen: status, devices, the pairing offer as a QR code, Send; asks for `POST_NOTIFICATIONS` at launch and sends the clipboard when it gains focus                                                   |
| `SendClipboardActivity`, `SendClipboardTileService` | A transparent activity that sends once it has focus, and the quick-settings tile that starts it                                                                                                              |
| `ShareActivity`                                     | The share sheet target: sends the shared text or streams to a chosen device, without touching the clipboard                                                                                                  |
| `ClipboardProvider`                                 | The `ContentProvider` serving announced parts and files; `openTypedAssetFile` answers a thumbnail request with a placeholder, `openFile` fetches                                                             |
| `NotificationListener`                              | The listener that emits the phone's notifications and will serve media sessions                                                                                                                              |
| `Transfers`                                         | One progress notification per transfer with Cancel, then Sent or Received                                                                                                                                    |

The Rust side (`ligature-android/src`): `lib.rs` (the `Host` trait the app implements and
the exported types), `node.rs` (the `Node` the Kotlin calls, a Tokio runtime,
`with_runtime`), `clipboard.rs` (`HostClipboard`), `notifications.rs`, `files.rs`
(MediaStore pending rows written through `/proc/self/fd`), `logging.rs` (the log to the
host) and `error.rs`. The Kotlin bindings are generated into `app/build/generated/uniffi/`
and never committed; a change to an exported type or function regenerates them on the next
Gradle build.

## What Android allows, and the app's answers

- The clipboard is readable only by a focused app or the keyboard. Receiving is automatic;
  sending happens when the app or the tile's activity gains focus, and automatically from
  the service only with `READ_CLIPBOARD_IN_BACKGROUND` on a platform-signed build, which an
  official LineageOS build cannot be (`NodeHolder` checks the permission before reading).
  Nothing else reads it: the share sheet sends what was shared.
- Files land in MediaStore's Downloads as pending rows, finished after the hash check; a
  clashing name gets a numbered suffix chosen by the app (`Transfers.freeName`), since
  MediaStore's own renaming is not relied on. The clipboard's files are fetched into
  Downloads by the Save action on the files notification.
- Notifications need `POST_NOTIFICATIONS` on 13 and later, asked for by `MainActivity` at
  launch; the foreground service needs its type declared; battery optimisation must be off
  for the node to live in the background.
- New permissions are opt-in from the app's own settings (milestone 6 adds SMS, calls and
  contacts that way): never requested at first run, and the capability's descriptor reflects
  what is held so the manifest changes live.
- A content URI served by `ClipboardProvider` is the way another app pastes an image or a
  file; `openFile` fetches on first open and must never fetch for a thumbnail.

## Compose conventions

Material 3 components, one screen in `ui/`, state hoisted from `NodeHolder`,
`safeDrawingPadding` on the root, user-visible strings in `res/values/strings.xml` (one
language today). Keep the screen plain; the app is a host for the node, not a product
surface yet.

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
- Send text as if shared from another app (quoted as one shell string):

  ```sh
  adb shell "am start -a android.intent.action.SEND -t text/plain \
    --es android.intent.extra.TEXT 'words' -n org.ligatureprotocol.android/.ShareActivity"
  ```

- `adb logcat -s ligature` for the node's log; `debug_logging` in the node config raises it.
- If adb says `unauthorized`, `adb kill-server` makes the phone prompt again. The owner's
  phone is a Pixel 8 Pro on LineageOS and enumerates on one USB port only; see the device
  notes in memory.

## Tests

`cargo test -p ligature-android --test two_nodes` runs the node end to end with a
`FakeHost` on real sockets; it is the regression test for anything in `node.rs`.

The Kotlin's JVM unit tests live in `app/src/test/kotlin` (JUnit 4, against android.jar's
stubs, which return defaults rather than throw): `./gradlew testDebugUnitTest`, and
`./gradlew kotlinCoverage` for the line coverage of the app's own Kotlin (the UniFFI bindings
and Compose's generated classes left out) under `app/build/reports/coverage/kotlin`. Logic
worth a test is kept out of Android classes so a JVM test reaches it: `coalesceTouch` in
`Touch.kt`, `Transfers.freeName`, the encoder's settings in `ScreenShareService`'s companion,
`remoteEdit`. Most of the app is services and Compose screens a JVM test cannot drive, so a
change there is still checked on the phone and recorded on the ticket.
