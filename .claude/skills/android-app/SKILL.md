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

| Kotlin                                                       | Role                                                                                                                                                                                                                                                                                                    |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `LigatureApp`                                                | The `Application`; registers nothing but the process                                                                                                                                                                                                                                                    |
| `NodeHolder`                                                 | The process-wide node, created once on first use with the data and cache directories, and the app's state: the clipboard reads, the files notification with its Save action, the host callbacks                                                                                                         |
| `NodeService`                                                | Foreground service of type `connectedDevice`, holds the multicast lock, started at boot and after an update (`BootReceiver`) and when the app opens, retrying a failed start; reads the clipboard itself only when `READ_CLIPBOARD_IN_BACKGROUND` is held                                               |
| `MainActivity`, `ui/MainScreen`, `ui/QrCode`                 | The one screen: status, devices, the pairing offer as a QR code, Send; asks for `POST_NOTIFICATIONS` at launch and sends the clipboard when it gains focus                                                                                                                                              |
| `SendClipboardActivity`, `SendClipboardTileService`          | A transparent activity that sends once it has focus, and the quick-settings tile that starts it                                                                                                                                                                                                         |
| `ShareActivity`                                              | The share sheet target: sends the shared text or streams to a chosen device, without touching the clipboard                                                                                                                                                                                             |
| `ClipboardProvider`                                          | The `ContentProvider` serving announced parts and files; `openTypedAssetFile` answers a thumbnail request with a placeholder, `openFile` fetches                                                                                                                                                        |
| `NotificationListener`                                       | The listener that emits the phone's notifications and will serve media sessions                                                                                                                                                                                                                         |
| `Transfers`                                                  | One progress notification per transfer with Cancel, then Sent or Received                                                                                                                                                                                                                               |
| `DriveService`                                               | The accessibility service the user turns on: carries out drives (gestures, the service's input connection, global actions, the pointer overlay), sees SystemUI's copy indicator and reads the clipboard for it, with follow-up reads while the indicator hides copies                                   |
| `ClipboardFocus`, `ClipEchoGuard`                            | A one-pixel focusable accessibility overlay that takes the focus long enough to read the clipboard, and the guard that tells the node's own clipboard sets from the user's copies                                                                                                                       |
| `ScreenShare`, `ScreenConsentActivity`, `ScreenShareService` | A viewer's request as a notification or Android's dialog (closed again when the request is withdrawn), the projection's foreground service and its encoder; `unattended` reads the owner's `PROJECT_MEDIA` grant                                                                                        |
| `ScreenWake`, `WatchDecoder`, `Touch`, `Zoom`                | The screen kept on for a drive or a share; a desktop's share decoded into a texture on the watch screen, zoomed by `Zoom`; touch coalescing for the touchpad and the drive                                                                                                                              |
| `CameraShare`, `CameraStartActivity`, `CameraShareService`   | A viewer's request for a camera, or the user's cast, started through a transparent activity (Android starts a camera only for an app in front); Camera2 into the H.264 encoder's surface and the microphone into Opus (`PlaybackSound` without a projection); the Camera card asks for the camera, once |

The Rust side (`ligature-android/src`): `lib.rs` (the `Host` trait the app implements and
the exported types), `node.rs` (the `Node` the Kotlin calls, a Tokio runtime,
`with_runtime`), `clipboard.rs` (`HostClipboard`), `input.rs` (`HostInput`), `notifications.rs`, `files.rs`
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
  With `DriveService` on it is automatic on any ROM: the listener does not fire out of focus, so
  the service takes SystemUI's `FrameLayout` window event with no text or one item and no
  content change type as the copy (the shade opens with many and closes as a pane that
  disappears), and `ClipboardFocus` adds a focusable accessibility overlay to read. A copy while
  the indicator is up raises nothing, so each read is followed by another about seven seconds
  later while the clip's `ClipDescription` timestamp keeps changing; a driven Control+C reads at
  once. A copy on the owner's phone reaches their real clipboards even off the LAN, through the
  peers' `get` on reconnect: test copies there only with the owner's go-ahead.
- Files land in MediaStore's Downloads as pending rows, finished after the hash check; a
  clashing name gets a numbered suffix chosen by the app (`Transfers.freeName`), since
  MediaStore's own renaming is not relied on. The clipboard's files are fetched into
  Downloads by the Save action on the files notification.
- Injecting input is possible only as an accessibility service (`DriveService`, turned on by
  the user): `dispatchGesture` for touch, one gesture at a time since a new one cancels the
  last, and the service's input connection (API 33) for text and keys. `HostInput` in the
  crate translates events to `DriveAction`s, so the logic is tested in Rust. The service also
  lifts the limit on opening an activity from the background. It must never press a consent
  dialog's button. The phone's `enabled_accessibility_services` lists other apps' services
  too: append to it over adb, never replace it.
- Notifications need `POST_NOTIFICATIONS` on 13 and later, asked for by `MainActivity` at
  launch; the foreground service needs its type declared; battery optimisation must be off
  for the node to live in the background.
- A camera is opened only by the app, behind the `CAMERA` permission, asked for from the
  Camera card and nowhere else; until it is held the node is told the phone shares none. A
  foreground service of the camera type starts only from an app the user can see, so a
  request from another device goes through `CameraStartActivity` (opened from the background
  only while the accessibility service is on) or a notification to tap. The picture is the
  sensor's, which lies along the phone's long side: nothing rotates it. Android has no
  camera an app can offer to other apps, so the phone never presents one. Never start the
  owner's camera over adb: that run is theirs.
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

- `uiautomator dump` suspends every accessibility service while it runs, so with Ligature's
  drive service on, a dump unbinds it for a moment: the app shows "not drivable" in that
  instant and the node advertises `accepts=false` then `true`. Read Ligature's own screen from
  a screenshot instead.
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
`CameraShare.fitSize` and `rateRange`, `cameraCastActions`,
`remoteEdit`. Most of the app is services and Compose screens a JVM test cannot drive, so a
change there is still checked on the phone and recorded on the ticket.
