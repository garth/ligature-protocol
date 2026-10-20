# Ligature for Android

The Android app of milestone 2: the Rust daemon (`ligatured`) built as a library
(`crates/ligature-android`) and wrapped in a Kotlin app that supplies what Android keeps
from a library: the clipboard, a foreground service, the Wi-Fi multicast lock, the device
name and a screen for the pairing ceremony. LineageOS is the first target: no Google
services, installed by sideloading, and optionally signed with the ROM's platform key so
clipboard sync is automatic in both directions (below).

## Build

Needs a JDK (17 or newer), a Rust toolchain with the Android targets, `cargo-ndk`, and the
Android SDK with NDK r27. With [mise](https://mise.jdx.dev) the JDK and Rust come from
`mise.toml` in this directory:

```sh
cd impl/android
mise install                       # JDK 21 and Rust stable
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --locked
echo "sdk.dir=$HOME/Android/Sdk" > local.properties   # where the SDK is
./gradlew assembleDebug
```

Gradle runs `cargo ndk` for `arm64-v8a` and `x86_64` (release profile) and then
`uniffi-bindgen` to generate the Kotlin bindings, before compiling the app. The NDK named
in `app/build.gradle.kts` is downloaded by the Android Gradle plugin if it is missing
(`sdkmanager --licenses` must have been accepted). Without mise, set `rust.cargo` in
`gradle.properties` (or `-Prust.cargo=/path/to/cargo`) if `cargo` is not on `PATH`.

The debug APK is `app/build/outputs/apk/debug/app-debug.apk`:

```sh
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

## First run

1. Open Ligature. It starts the node as a foreground service (a persistent notification)
   and asks for notification permission on Android 13 and later. Allow unrestricted
   battery use when the screen offers it, or Android will stop the node in the background.
2. On a device that already has your keys, nothing to do yet. On the phone, tap **Show
   pairing offer**. Send the offer text to that device (**Share** or **Copy**, or scan the
   QR code with anything that can read one) and run there:

   ```sh
   lpctl pair 'ligature:pair/1?…'
   ```

3. Both show a six-digit code. Confirm on both. The phone is now one of your devices and
   every other device learns of it by gossip.

If the phone is your first device, enter your name and tap **Create my keys on this
phone** instead; it then holds your master key and can introduce the others.

## Clipboard

Receiving needs nothing: text copied on your other devices lands on the phone's clipboard
while the node runs. Sending depends on what Android allows:

- **Without special permission** (any ROM): Android 10 and later let an app read the
  clipboard only while it has focus or is the keyboard. The phone therefore sends on
  request: the **Send clipboard** quick-settings tile, the action on the notification,
  **Send clipboard now** on the main screen, or **Send to my devices** in the share sheet.
  Opening the app also sends what is on the clipboard.
- **Platform-signed build** (a LineageOS you build yourself, or any ROM whose platform key
  you hold): the app declares `android.permission.READ_CLIPBOARD_IN_BACKGROUND`, which is
  granted only to apps signed with the platform certificate. With it the primary-clip
  listener fires in the background and every copy is sent as it happens. Build the
  unsigned release APK and sign it with the ROM's key:

  ```sh
  ./gradlew assembleRelease
  apksigner sign --key platform.pk8 --cert platform.x509.pem \
    --out ligature-platform.apk app/build/outputs/apk/release/app-release-unsigned.apk
  adb install -r ligature-platform.apk
  ```

  `platform.pk8` and `platform.x509.pem` are the keys the ROM was signed with
  (`build/make/target/product/security/` in a self-built tree, or the keys made with
  `make_key`). The main screen reports "automatic (background read allowed)" when the
  permission is held.

## Where things are

| | |
| --- | --- |
| Node, keys, store | `files/ligature/` in the app's private storage (`keys.cbor` 0600, `store.sqlite`) |
| Logs | logcat, tag `ligature` |
| Device name | the name under Settings → About phone, else the model |
| Service | `NodeService`, foreground type `connectedDevice`, started at boot and when the app opens |
| Bindings | generated into `app/build/generated/uniffi/`, never committed |
