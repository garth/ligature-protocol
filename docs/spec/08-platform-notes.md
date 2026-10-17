# 08. Platform Notes

**Status:** Draft.

## 1. Intent

Each platform constrains what a device can offer and how it can run. This chapter records
those constraints so capability specs do not assume more than a platform allows.

## 2. Linux (Wayland)

The reference daemon (`ligatured`) runs as a systemd user service and exposes D-Bus
interfaces. Clipboard, notifications and remote input on Wayland go through compositor
protocols and portals (for example `wlr-data-control`, `xdg-desktop-portal`), which differ
by compositor. Open question: the minimum set of compositors and portals to support
(Hyprland is the first target).

## 3. Android

Background execution, battery optimisation and the clipboard restrictions introduced in
Android 10 shape what an LP app can do. Notification access, SMS and telephony require
specific permissions and, for SMS, Play Store policy approval. Open question: whether the
Android reference app targets Play Store distribution or F-Droid first.

## 4. iOS

Third-party apps cannot run a general background daemon. LP's iPhone support relies on
BLE accessory profiles (ANCS for notifications, AMS for media, HFP/MAP/PBAP for telephony
and contacts via a Linux device acting as an accessory) and on EU Digital Markets Act
interoperability APIs where they exist. Open question: which features are realistic with
no app installed versus with a companion app.

## 5. Home hubs and headless devices

Hubs have no screen or camera, so pairing and recovery flows need an alternative to QR
codes. Open question: whether a trusted device can pair a hub on its behalf.

## 6. Open questions

- Which platforms must be supported by the first public release?
- How are platform-specific capability limitations expressed in descriptors?
