# Notifications

A notification that appears on one device appears on the others, with the app's name and
icon and the device it came from, and dismissing it anywhere clears it everywhere the
platform allows. Each platform differs in what it can see of its own notifications:

| Device | Shows the others' notifications | Sends its own |
| --- | --- | --- |
| Linux | Yes, through the desktop's notification service | Yes, by watching the session bus |
| macOS | Yes, without icons | No: macOS lets no third party read them |
| Android | Yes | Yes, once you grant notification access |

## Linux

Nothing to set up. The daemon displays what your other devices post through whatever
notification daemon your desktop runs, and sees every notification your own applications
post by monitoring the session bus, so a message that pops up on the desktop reaches the
phone and the Mac. Closing a copy on another device closes the original here, and closing
the original closes the copies.

## macOS

The Mac displays only. Its notifications are posted through AppleScript, which means:

- they carry no icon and cannot be updated or closed from outside, so a dismissal made on
  another device does not clear them;
- they post as Script Editor, and show only after you have allowed Script Editor to post
  once (see [Install](install.md#macos));
- a Focus mode hides them like any other.

Nothing an application posts on the Mac leaves it, because macOS gives no process a view of
other applications' notifications.

## Android

1. Tap **Notification access** on the app's screen and enable Ligature in Settings. Until
   you do, the phone displays your other devices' notifications but sends none of its own;
   the screen says "displaying only (no notification access)".
2. Apps appear in the **Notifications** card as they post, each with a switch. Turn an app
   off to keep its notifications on the phone; the others still go out. The switch is
   remembered.
3. Ongoing, silent and summary notifications stay on the phone, as do Ligature's own.

Your other devices' notifications arrive grouped by the device they came from, with the app
and device named beneath. Swiping one away dismisses it on the device that posted it and
on your other devices.

## Trying it

From a desktop:

```sh
lpctl notify "Hello" "from the desk"
```

posts a notification from this device to every device that displays them, as an application
would.

## Privacy

Notifications travel only between your own devices, over the encrypted session each pair of
them holds, and are never stored by the daemon: a copy exists while the original does.
Icons travel once per session and are checked against their hash on arrival.
