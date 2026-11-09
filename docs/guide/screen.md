# Screens

You can watch another of your devices' screens in a window and, with a click, drive it with
your own keyboard and mouse. The picture goes straight between the two devices, encoded as
H.264 on the device that shares and decoded on the one that watches. The phone can watch and
drive a desktop, share its own screen with one, and be driven from the picture once
Ligature is on under Accessibility.

From the release after 0.2.0-alpha you can also go the other way, and **cast** a screen or
one window from the device you are at to another, and a share carries the sharing device's
**sound** ([Casting](#casting-a-screen-or-a-window), [Sound](#sound)).

## Watching a desktop from a desktop

```sh
lpview Garths-MacBook-Pro
```

The device is named as `lpctl devices` shows it. A window opens with the other screen's picture
scaled to fit. **Click the picture** to drive the device: the pointer follows yours over the
picture, and your buttons, scroll wheel and keyboard go there. Your keys type what your own
keyboard says, as when driving across an edge ([Keyboard and
mouse](input.md#what-your-keys-type)). To take them back, press **both Control keys**, or an
**Alt or Super key on each side** (Option or Command on a Mac), or move to another window. Closing
the window ends the
share.

| Option | What it does |
| --- | --- |
| `--display <id>` | Shares that display instead of the primary one; `lpctl screen displays <device>` lists them |
| `--width`, `--height`, `--rate` | The largest picture and the frame rate you want (1920 by 1080 at 60 a second by default); the sharer sends no larger than its own screen |
| `--actual-size` | Shows the screen at its actual size instead of scaled to the window (below); `--width` and `--height` are then ignored |
| `--chroma 4:2:0` or `4:4:4` | Asks for that colour sampling only (below); by default the best both devices handle |
| `--view-only` | Watch without ever driving |
| `--window <id>` | Watches one window instead of a display, where the device lists windows (a Mac); a window is never driven |
| `--mute` | The picture alone, without the device's sound |

A device that takes no input, such as the phone before Ligature is on under Accessibility,
is only watched, and the window's title says why.

### At its actual size

Scaling a picture to the window softens text. With `--actual-size`, the other screen is
shown at the size it has on that device: a window there is the same size in `lpview`, even
when one screen is high-DPI and the other is not. The picture is sent at that size, so
`lpview` draws it pixel for pixel. The one exception is a high-DPI viewer of a screen that is
not; there each of its pixels becomes a square of two by two or more, since the other device
has no more pixels to send.

When the picture is smaller than the window, it sits in black margins. When it is larger,
it scrolls:

- **while not driving**, with the scroll wheel, and Shift with the wheel for sideways;
- **while driving**, by holding the pointer at an edge of the window, since the wheel
  then scrolls on the other device.

`lpctl screen displays <device>` shows each display's size in pixels and its scale, for
example `2880x1800 at 2x` for a high-DPI laptop screen.

### Colour detail

Video usually keeps colour at half the resolution of brightness ("4:2:0"), which blurs the
edges of coloured text. When both devices can handle it, `lpview` asks for full colour
detail ("4:4:4"), and the window's title shows which one the share uses and its frame rate.
A Linux machine encoding with `libx264` sends 4:4:4. A Mac encoding with VideoToolbox, and
the phone, send 4:2:0, since their hardware encoders do not produce 4:4:4. 4:4:4 takes
about half as much again of the network, so on a slow link `--chroma 4:2:0` asks for the
lighter one.

## Casting a screen or a window

Watching starts at the device that shows the picture. Casting starts at the device that
has it: you pick what to show and where, and it opens there by itself.

```sh
lpctl screen cast Garths-MacBook-Pro            # this device's primary display
lpctl screen cast Garths-MacBook-Pro --window   # one window
lpctl screen cast "Pixel 8 Pro" --no-audio      # the picture without this device's sound
```

The command says when the other device has taken the cast and what it settled on, and runs
until the cast ends. **Ctrl+C** leaves the cast running; `lpctl screen stop` ends it, as
closing the picture on the other device does.

- **On Linux**, `--window` opens the screen-sharing picker so you choose the window there.
  Linux has no list of windows a program may read, so a window is chosen each time and is
  never remembered.
- **On a Mac**, `lpctl screen displays` with no device lists this Mac's displays and
  windows with their ids, and `--window <id>` casts one.
- **On the phone**, tap **Cast to** followed by the device's name under **Cast this screen**.
  Android's dialog opens at once and decides what is shown: one app, or the entire screen.
- A window keeps its shape in the picture: resize it and it is fitted in with black borders.

**On the receiving device** the picture opens without anyone asking for it: `lpview` on a
desktop, the watch screen on the phone. A cast display can be driven from there with a
click, as a watched one can; a cast window is only shown. On the phone the cast opens at
once while the app is open or Ligature is on under Accessibility, and is otherwise a
notification, *device* **is casting to this phone**: tap it to watch, swipe it away to
decline. A cast nobody takes within a minute is declined, and the casting device says so.

A device shows casts only when it has something to show them in: a desktop needs `lpview`
installed beside the daemon, as the packages do, and ffmpeg. `lpctl screen status` says
`receives: true` on a device that does, and lists a cast that is waiting to be taken.

## Sound

A share, watched or cast, carries what the sharing device plays, and the watching device
plays it with the picture. `lpview --mute` and the watch screen's **Mute** silence it on the
watching side; `lpctl screen cast --no-audio` sends none.

- **Linux** shares what plays on its default output, and needs ffmpeg built with `libopus`
  and PipeWire, as desktop distributions have.
- **A Mac** shares its sound on macOS 13 and later. Before that Apple gives a program no
  way to what the Mac plays, so an older Mac shares the picture alone.
- **The phone** shares its sound only once you allow recording: under **Cast this screen**,
  tap **Share the phone's sound too**. Android puts what the phone plays behind the same
  permission as the microphone. A share of the screen never opens the microphone; only a
  camera you share carries it ([Cameras](cameras.md)). Android leaves calls out of every
  share, and any app that asks not to be captured, so some sound never leaves the phone
  whatever you allow.
- Watching a device that is in the same room plays its sound twice, once from each. Mute
  one of them.

## What the sharing device asks

- **Linux** shows its screen-sharing picker the first time a device asks to watch. It
  remembers your choice, so later shares start without asking. While a share runs, a
  notification "Sharing *display* with *device*" stays up ("Casting", for a cast you
  started), and its **Stop sharing** button ends the share. That notification is the only
  sign on Linux that the screen is shared,
  since the compositor shows none, so a share does not start when it cannot be shown, and
  dismissing it brings it back a few times before it stays dismissed; **Stop sharing** is the
  way to end the share. A notification daemon that hides popups after a while keeps it in
  its list.
- **The Mac** asks once for permission to record the screen, and refuses that first share;
  once you allow it, the daemon restarts by itself and the next share works
  ([Install](install.md#macos) has the steps). While a share runs, macOS shows its own
  sharing indicator in the menu bar, and stopping the share there ends it.
- **The phone** asks every time, since Android never lets an app keep that consent, unless
  you grant it once from a computer ([below](#watching-the-phone-with-nobody-at-it)). The
  request shows as a notification, **Tap to choose whether to share it**, or straight away as
  Android's dialog while the app is open or Ligature is on under Accessibility
  ([Driving the phone](input.md#driving-the-phone)). Someone must answer it at the phone,
  because Ligature never presses it for you. Choose **Share entire screen**, since Android
  offers one app first. If the phone is locked, Android asks for its PIN, pattern or
  password before the dialog, and that screen cannot be seen from the viewer. A request
  nobody answers within a minute is withdrawn and the dialog closes. While the screen is
  shared, the notification's **Stop sharing** ends it.

### Watching the phone with nobody at it

Android has no setting for letting an app share the screen without its dialog. A computer
with `adb` (Android's platform tools) and the phone's USB debugging on (Settings, System,
Developer options, which appear after tapping Build number seven times) can grant it once:

```sh
adb shell appops set org.ligatureprotocol.android PROJECT_MEDIA allow
```

With that and Ligature on under Accessibility, a request to watch wakes the phone and shares
at once, and **Remote control** on the app's screen says your devices can watch with nobody
there. Every device you have paired can then watch the phone without asking, a lost one too
until you revoke it, so grant it only if you want that. The phone still shows that it is
sharing, and **Stop sharing** still ends a share. A locked phone shares its lock screen, but
not its PIN, pattern or password screen, which Android keeps out of every share: the picture
dims while it is up. Clicks still reach it, so a PIN can be entered from the viewer by its
place on the pad. To ask every time again:

```sh
adb shell appops set org.ligatureprotocol.android PROJECT_MEDIA ignore
```

## Watching a desktop from the phone

On the app's main screen, under **Watch a screen**, tap **Watch** followed by the desktop's
name. A desktop wider than it is tall turns the phone's screen to landscape while you watch
it, and the picture fills the screen. Touch drives the desktop:

- a tap clicks where it lands, and a drag moves the pointer there;
- a touch held still and then moved drags with the button down;
- two fingers moved together scroll;
- pinching zooms the picture, up to five times, and moving the fingers while pinching pans
  it; taps land where they touch at any zoom;
- the field types into the desktop;
- **Done**, or leaving the screen, ends the watch and returns the screen to how it was.

## Stopping and checking

```sh
lpctl screen status             # what this device shares, casts, watches and is offered
lpctl screen stop               # stop every share this device runs
lpctl screen stop "Pixel 8 Pro" # or only those with one device
lpctl screen displays           # this device's own displays, and its windows on a Mac
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

It takes `--display`, `--window`, `--width`, `--height` and `--rate` as `lpview` does, asking
for 1280 by 720 at 60 a second by default. `--audio-out sound.bin` asks for the sound too and
writes its Opus packets to that file, for checking that sound arrives.

## What each platform needs

- **Linux and the Mac** need ffmpeg to share or to watch, and Linux needs GStreamer's
  PipeWire plugin to share through the screen-sharing portal ([Install](install.md#linux)).
  Without ffmpeg a desktop offers neither; `lpctl screen status` on the device itself says
  whether it shares and views, and `lpctl caps` on another device lists the family.
- **The phone** needs an H.264 encoder to share, which phones have. A request opens Android's
  dialog straight away only while the app is open or Ligature is on under Accessibility, and
  is answered with nobody at the phone only after the grant in
  [Watching the phone with nobody at it](#watching-the-phone-with-nobody-at-it).
- **Driving** a desktop from the picture needs that desktop to accept input, as in
  [Keyboard and mouse](input.md#what-each-platform-needs), and driving the phone needs Ligature
  on under Accessibility ([Driving the phone](input.md#driving-the-phone)).

## Limits

- Screens are shared between your own devices only.
- One device watches a display at a time: a second viewer of the same display is refused as
  busy. Another display of the same device can be shared alongside.
- Up to 0.2.0-alpha a whole display is shared, never a single window, and a share carries
  no sound. After it, a window is shared from a Mac by its id and from Linux as a cast, a
  window is never driven, and the phone shares what Android's dialog gives it.
- A locked phone shares its lock screen but not its PIN, pattern or password screen, which
  is entered unseen from a viewer.
- Sound comes only with a picture. Music from the phone on a desktop's speakers alone is not
  something the protocol carries yet; a microphone goes with a camera
  ([Cameras](cameras.md)).
- A cast goes to your own devices. A television or another person's device needs the
  household groups the protocol specifies and the apps do not have yet.
