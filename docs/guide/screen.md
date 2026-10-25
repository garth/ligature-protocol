# Screens

You can watch another of your devices' screens in a window and, with a click, drive it with
your own keyboard and mouse. The picture goes straight between the two devices, encoded as
H.264 on the device that shares and decoded on the one that watches. The phone can watch and
drive a desktop, and can share its own screen with one.

## Watching a desktop from a desktop

```sh
lpview Garths-MacBook-Pro
```

The device is named as `lpctl devices` shows it. A window opens with the other screen's
picture scaled to fit. **Click the picture** to drive the device: the pointer follows yours
over the picture, and your buttons, scroll wheel and keyboard go there. Your keys type what
your own keyboard says, as when driving across an edge
([Keyboard and mouse](input.md#what-your-keys-type)). To take them back, press **both Control
keys**, or move to another window. Closing the window ends the share.

| Option | What it does |
| --- | --- |
| `--display <id>` | Shares that display instead of the primary one; `lpctl screen displays <device>` lists them |
| `--width`, `--height`, `--rate` | The largest picture and the frame rate you want (1920 by 1080 at 30 a second by default); the sharer sends no larger than its own screen |
| `--view-only` | Watch without ever driving |

A device that takes no input, such as the phone, is only watched.

## What the sharing device asks

- **Linux** shows its screen-sharing picker the first time a device asks to watch. It
  remembers your choice, so later shares start without asking. While a share runs, a
  notification "Sharing *display* with *device*" stays up, and its **Stop sharing** button
  ends the share.
- **The Mac** asks once for permission to record the screen, and refuses that first share
  ([Install](install.md#macos) has the steps). While a share runs, macOS shows its own
  sharing indicator in the menu bar, and stopping the share there ends it.
- **The phone** asks every time, since Android never lets an app keep that consent. The
  request shows as a notification, **Tap to choose whether to share it**, or straight away as
  Android's dialog while the app is open. While the screen is shared, the notification's
  **Stop sharing** ends it.

## Watching a desktop from the phone

On the app's main screen, under **Watch a screen**, tap **Watch** followed by the desktop's
name. The picture fills the screen, and touch drives the desktop:

- a tap clicks where it lands, and a drag moves the pointer there;
- a touch held still and then moved drags with the button down;
- two fingers scroll;
- the field types into the desktop;
- **Done**, or leaving the screen, ends the watch.

## Stopping and checking

```sh
lpctl screen status             # what this device shares and watches
lpctl screen stop               # stop every share this device runs
lpctl screen stop "Pixel 8 Pro" # or only those with one device
```

A share also ends when the watching device closes its window or loses its connection. If the
sharer's capture stalls, it is started again by itself within a few seconds, so the picture
pauses and carries on.

`lpctl screen start` writes the stream to a file instead of a window, which is a way to check a
share without `lpview`. It runs until **Ctrl+C**, or for as many frames as `--frames` says:

```sh
lpctl screen start Garths-MacBook-Pro --out share.h264 --frames 300
ffplay -f h264 share.h264
```

## What each platform needs

- **Linux and the Mac** need ffmpeg to share or to watch, and Linux needs GStreamer's
  PipeWire plugin to share through the screen-sharing portal ([Install](install.md#linux)).
  Without ffmpeg a desktop offers neither, and `lpctl caps` on another device shows what it
  offers.
- **Driving** a desktop from the picture needs that desktop to accept input, as in
  [Keyboard and mouse](input.md#what-each-platform-needs).

## Limits

- Screens are shared between your own devices only.
- One device watches a display at a time: a second viewer of the same display is refused as
  busy. Another display of the same device can be shared alongside.
- A whole display is shared, never a single window.
- There is no sound.
