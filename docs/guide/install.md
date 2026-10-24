# Install

Every [release](https://github.com/garth/ligature-protocol/releases) carries one package
per platform:

| File | For |
| --- | --- |
| `ligature-<version>-linux-x86_64.tar.gz` | Linux on an Intel or AMD machine |
| `ligature-<version>-linux-aarch64.tar.gz` | Linux on an ARM machine |
| `ligature-<version>-macos-universal.tar.gz` | Any Mac, Intel or Apple silicon |
| `ligature-<version>-android.apk` | Android 10 or later |

Install the same version on every device: the protocol is still changing between releases.

## Linux

The daemon runs as a systemd user service and expects a Wayland session for the clipboard
(Hyprland is the first target; any compositor with the `wlr-data-control` protocol should
work) and a notification daemon on the session bus for notifications.

1. Unpack the tarball and run the installer:

    ```sh
    tar xzf ligature-<version>-linux-x86_64.tar.gz
    cd ligature-<version>-linux-x86_64
    ./install.sh
    ```

    It copies `ligatured`, `lpctl` and `lpview` to `~/.local/bin`, installs the `ligatured`
    user unit and starts it. Make sure `~/.local/bin` is on your `PATH`.

2. Open the port. The service fixes the daemon's UDP port at 47471 so that one firewall rule
    lets your other devices dial in. With ufw:

    ```sh
    sudo ufw allow 47471/udp
    ```

    Without the rule the machine can still dial your other devices, but they cannot dial it,
    so it connects only when it finds them first.

3. Check it:

    ```sh
    lpctl status
    journalctl --user -u ligatured -f
    ```

The daemon does not need a graphical session to run: it finds the compositor's socket on its
own, advertises the clipboard only while a compositor is reachable, and keeps gossiping and
receiving files when none is.

Sharing the screen and watching another device's with `lpview` use programs the installer
does not bring, since the daemon links no video library: ffmpeg with an H.264 encoder
(`libx264`, or VA-API where the GPU has it) to share and to watch, and GStreamer's PipeWire
plugin (`gst-plugin-pipewire` on Arch) to share through the ScreenCast portal. Without the
plugin a compositor with `wlr-screencopy`, Hyprland among them, is captured directly instead;
without ffmpeg the machine offers neither, and `lpctl caps` shows what it offers.

## macOS

The daemon runs as a per-user launchd agent that starts at login.

1. Unpack the tarball and run the installer:

    ```sh
    tar xzf ligature-<version>-macos-universal.tar.gz
    cd ligature-<version>-macos-universal
    ./install.sh
    ```

    It copies `lpctl` and `lpview` to `~/.local/bin` and the daemon, inside
    `Ligature.app`, to `~/Applications`, writes
    `~/Library/LaunchAgents/org.ligatureprotocol.ligatured.plist` and loads it. The port is
    fixed at 47471 here too; if the macOS firewall is on, allow Ligature when it asks.

2. The binaries and the bundle are signed ad hoc, not notarised. If macOS refuses to run
    them after a browser download, clear the quarantine flag on the unpacked directory and
    run the installer again:

    ```sh
    xattr -dr com.apple.quarantine .
    ```

3. To watch another device's screen with `lpview`, install ffmpeg (`brew install ffmpeg`).
    The Mac does not share its own screen yet.

4. Allow notifications once. When the daemon first starts, macOS asks whether Ligature may
    send notifications; click **Allow**, and your other devices' notifications show from
    then on.

5. Allow input sharing if you want it. When the daemon first starts, macOS also asks about
    Input Monitoring and Accessibility for Ligature; both are under System Settings, Privacy
    & Security. With Accessibility allowed your other devices can drive the Mac's pointer and
    keyboard; with Input Monitoring as well, the Mac's own keyboard and mouse can drive
    theirs. Until then the Mac leaves input alone and offers none.

6. Check it:

    ```sh
    lpctl status
    tail -f ~/Library/Logs/ligature/ligatured.log
    ```

## Android

1. Allow installs from the source you use (your browser or file manager) and open the APK.
    The release APK is signed with the project's key; an update installs over the previous
    version and keeps your identity.
2. Open Ligature. It starts the node as a foreground service, which shows a persistent
    notification, and asks to post notifications on Android 13 and later: allow it, since
    transfers, received files and the clipboard's **Save** all arrive as notifications.
3. Tap **Battery settings** when the screen offers it and allow unrestricted battery use.
    Without it Android stops the node when the phone has been idle for a while.
4. For your other devices' notifications to leave the phone, tap **Notification access** and
    enable Ligature; see [Notifications](notifications.md).

The app needs no account and asks for nothing else at first run.

## From source

With a Rust toolchain:

```sh
git clone https://github.com/garth/ligature-protocol
cd ligature-protocol/impl
cargo build --release -p ligatured -p lpctl -p lpview
../packaging/install.sh target/release
```

The Android app builds from `impl/android` with the Android SDK and NDK; its
[README](https://github.com/garth/ligature-protocol/blob/main/impl/android/README.md) has
the steps.

## Verifying a download

Every release also carries `SHA256SUMS`, the checksums of its four files, and
`SHA256SUMS.minisig`, the checksum file's signature by the project's release key, made with
[minisign](https://jedisct1.github.io/minisign/). The public key is:

```text
RWRG8kOII0tLS+Ll66WZ15knTjMWMnxqIxyRfwJRiMB64wBRsGxJy2BJ
```

With `minisign` (or `rsign`) installed, in the directory holding the downloads:

```sh
minisign -Vm SHA256SUMS -P RWRG8kOII0tLS+Ll66WZ15knTjMWMnxqIxyRfwJRiMB64wBRsGxJy2BJ
sha256sum --ignore-missing -c SHA256SUMS     # shasum -a 256 --ignore-missing -c SHA256SUMS on a Mac
```

The first command prints the trusted comment, `ligature-protocol v<version>`, which should
name the release you downloaded; the second prints `OK` for each file present. A release
whose checksum file does not verify is not the project's.

## Where things live

| | Linux | macOS | Android |
| --- | --- | --- | --- |
| Binaries | `~/.local/bin` | `~/.local/bin`, the daemon in `~/Applications/Ligature.app` | the app |
| Keys and store | `~/.local/share/ligature` | `~/Library/Application Support/ligature` | the app's private storage |
| Control socket | `$XDG_RUNTIME_DIR/ligature` | `$TMPDIR/ligature` | none |
| Logs | `journalctl --user -u ligatured` | `~/Library/Logs/ligature/ligatured.log` | logcat, tag `ligature` |
| Received files | `XDG_DOWNLOAD_DIR`, else `~/Downloads` | `~/Downloads` | Downloads |

The key file holds your private keys and is readable by your user alone. There is no
configuration file: `ligatured --help` lists the few options (`--data-dir`, `--runtime-dir`,
`--device-name`, `--port`), each also settable through an environment variable, and the
service sets `LIGATURE_PORT=47471`.
