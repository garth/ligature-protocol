# Using Ligature

Ligature binds the devices you own into one trusted group, so that what you copy, the
notifications you get and the files you share move between them directly, over your own
network, with no account and no server in between. The reference apps run on a Linux
desktop, a Mac and an Android phone; this guide covers installing them and using every
feature they have today.

## What you need

- **Devices on one network.** The apps find each other on the local network (mDNS) and talk
  to each other directly over QUIC. Nothing leaves the network, and nothing works across the
  internet yet; remote connections, relays and sharing with other people are specified but
  not built.
- **Linux with Wayland**, a **Mac**, or an **Android phone or tablet** on Android 10 or
  later. LineageOS is the first Android target; the app is installed by sideloading, not
  from a store. Apple phones and tablets are not supported.
- **Your devices in one place when you pair them.** Adding a device is an in-person
  ceremony: one of your devices shows an offer, another confirms a six-digit code with it.
  There is no remote joining.

## The three apps

| App | Platform | What it does |
| --- | --- | --- |
| `ligatured` | Linux, macOS | The daemon: runs as a user service, holds your keys and talks to your other devices |
| `lpctl` | Linux, macOS | The command line that drives the daemon: pairing, status, sending files, diagnostics |
| `lpview` | Linux, macOS | A window that shows another of your devices' screens and drives it |
| Ligature for Android | Android | The daemon inside an app, with a screen for pairing and the phone's own ways of sending |

## How the guide is organised

1. [Install](install.md) the apps on each device.
2. [First steps](first-steps.md): create your keys on one device and pair the others.
3. The features: the [clipboard](clipboard.md), [notifications](notifications.md),
   [files](files.md), one [keyboard and mouse](input.md) across your desktops, and
   [screens](screen.md) watched and driven from another device.
4. [Your devices](devices.md): seeing them, naming them, revoking one, starting over.
5. [Upgrading and uninstalling](upgrade-uninstall.md).
6. [Troubleshooting](troubleshooting.md) when devices do not find each other or something
   does not arrive.

Each page says what a platform cannot do as well as what it can: the Mac cannot read its own
notifications, the phone reads its clipboard only when you ask it to, and so on. Those are
limits of the platforms, recorded in the
[platform notes](../spec/08-platform-notes.md), not bugs to report.

## Status

Ligature is alpha. The releases work for the owner's three devices and are used daily, but
the protocol is a draft and the wire format may change between releases, so update every
device together. Problems go to the project's
[issue tracker](https://github.com/garth/ligature-protocol/issues); anything that looks like
a security problem goes through a
[security advisory](https://github.com/garth/ligature-protocol/security/advisories/new)
instead.
