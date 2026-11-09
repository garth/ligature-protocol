# Cameras

A camera on one of your devices can be used from another: watched in a window, or, on a
Linux machine, presented to its programs as a webcam, so a video call there uses the phone's
camera and microphone as if they were plugged in. The picture and the sound go straight
between the two devices, as a watched screen's do.

Cameras are on `main` and in no release yet, so they come after 0.2.0-alpha. They have run
between a Linux machine and an Android phone; the Mac's part is written and has not yet
been run on a Mac.

## Seeing which cameras a device has

```sh
lpctl camera list                    # this device's own cameras
lpctl camera list "Pixel 8 Pro"      # another device's
```

Each line is a camera's id, its size, which way it faces and its name; the first is the one
used when you name none. The device is named as `lpctl devices` shows it.

## Watching a camera

```sh
lpview "Pixel 8 Pro" --camera            # its first camera
lpview "Pixel 8 Pro" --camera 1          # the camera with that id
lpview "Pixel 8 Pro" --camera --mute     # without its microphone
```

A window opens with the camera's picture and plays the device's microphone. A camera is
watched, never driven, so clicking the picture does nothing. Closing the window ends the
share.

On the phone, the **Cameras** card on the app's main screen has **Watch** followed by the
device's name and **camera** for each device that shares one.

## Using another device's camera as a webcam

On a Linux machine:

```sh
lpctl camera present "Pixel 8 Pro"               # its first camera
lpctl camera present "Pixel 8 Pro" --camera 1    # the camera with that id
lpctl camera present "Pixel 8 Pro" --no-audio    # the picture without its microphone
```

While it runs, the machine has a camera called **Ligature camera (Pixel 8 Pro)** and a
microphone called **Ligature microphone (Pixel 8 Pro)**, and a program picks them where it
picks any camera and microphone. The command runs until the share ends. **Ctrl+C** leaves it
running; `lpctl camera stop` ends it, and the camera and the microphone go away.

- The camera is offered as a PipeWire camera, which a program lists when it finds cameras
  through PipeWire. A browser does where its PipeWire camera support is on: in Chromium
  that is the flag `enable-webrtc-pipewire-camera`, in Firefox the setting
  `media.webrtc.camera.allow-pipewire`. Which programs list it has not been checked yet. A
  program that reads cameras only through the older V4L2 interface does not see it unless
  the `v4l2loopback` kernel module is loaded. With the module, the same picture also goes
  to its first loopback device, which carries the name the module was loaded with:

  ```sh
  sudo modprobe v4l2loopback exclusive_caps=1 card_label="Ligature camera"
  ```

- Neither becomes the machine's default camera or microphone: you choose them in the
  program that is to use them.
- The microphone appears with an output beside it, **Ligature microphone feed**, which is
  where the sound is played in. Leave it alone: anything else played into it is heard in
  the microphone.
- If the sound of the call plays in the same room as the phone, its microphone hears it and
  the other side hears an echo. Mute one of them, or use `--no-audio` and the machine's own
  microphone.

**A Mac and the phone do not present a camera.** On a Mac, a camera for other programs is
a system extension that macOS loads only from an application Apple has notarised, for which
the developer pays Apple and submits each build. Android has no way at all for an app to
offer a camera to other apps. Both are the makers' choices about what you may do with your
own device, not limits of Ligature, which presents a camera wherever the system lets it. A
Mac and the phone still share their cameras, and show one that is cast to them.

## Casting a camera

Watching and presenting start at the device that shows the picture. Casting starts at the
device that has the camera:

```sh
lpctl camera cast deskbox                # shown in a window on deskbox
lpctl camera cast deskbox --present      # used as deskbox's webcam
lpctl camera cast deskbox --camera 1     # the camera with that id, from lpctl camera list
lpctl camera cast deskbox --no-audio     # the picture without this device's microphone
```

A cast to be shown opens in `lpview` on a desktop, or on the watch screen on the phone, as
a cast screen does ([Casting a screen](screen.md#casting-a-screen-or-a-window)). A cast
with `--present` needs a device that presents, and is taken there at once with nobody
asked, since it goes between your own devices.

On the phone, the **Cameras** card has, for each device, **Use as** *device***'s webcam**
where the device presents and **Show on** *device* where it shows casts. With more than one
camera, **Camera:** above them changes which is cast. **Stop the camera** ends it.

## What the sharing device asks and shows

- **Linux** asks nothing: a program that is not sandboxed may open a camera. So the daemon
  shows a notification, "Sharing *camera* with *device*" ("Casting", for a cast you
  started), for as long as the camera is on, and its **Stop sharing** button ends the
  share. A share does not start when the notification cannot be shown.
- **A Mac** asks once whether Ligature may use the camera, and once for the microphone,
  the first time one of your devices watches a camera. The share waits for your answer, up
  to half a minute. Unlike Screen Recording, the answer applies at once, with no restart.
  While a camera is on, macOS shows its own indicators: the light beside the camera and a
  dot in the menu bar. `lpctl permissions` shows both permissions, and `lpctl permissions
  open camera` opens the page where one is turned on.
- **The phone** shares no camera until you allow it: tap **Allow the camera** on the
  **Cameras** card, and Android asks for the camera and the microphone. After that your
  other devices can start a camera themselves. Android starts a camera only for an app you
  can see, so a request that comes while the app is in the background shows a notification,
  *device* **wants to see this phone's camera**, to tap; with Ligature on under
  Accessibility it starts by itself
  ([Driving the phone](input.md#driving-the-phone)). While a camera is on, Android shows
  its green dot, and the notification's **Stop sharing** ends it.

A share reads the camera it was asked for and never another in its place. If that camera
is unplugged, the share ends.

## Stopping and checking

```sh
lpctl camera status              # whether this device shares, shows and presents, and what runs
lpctl camera stop                # stop every camera share this device runs
lpctl camera stop "Pixel 8 Pro"  # or only those with one device
```

`lpctl camera start` writes the stream to a file instead of a window, to check that a
camera arrives:

```sh
lpctl camera start "Pixel 8 Pro" --out camera.h264 --frames 150
ffplay -f h264 camera.h264
```

## What each platform needs

- **Linux** needs ffmpeg and GStreamer's PipeWire plugin, as for screens
  ([Install](install.md#linux)), and PipeWire's own tools, which come with it: `pw-dump` to
  find the cameras, `pw-record` for the microphone and `pw-loopback` for a presented
  microphone, with `pactl` beside them.
- **A Mac** needs ffmpeg, as for screens ([Install](install.md#macos)).
- **The phone** needs nothing more than the two permissions.

## Limits

- Cameras are shared between your own devices only.
- Only a Linux machine presents a camera to its programs. A Mac and the phone cannot, for
  the reasons [above](#using-another-devices-camera-as-a-webcam).
- The phone's picture is its sensor's, which lies along the phone's long side: held upright
  it arrives on its side. Turn the phone on its side to use it as a webcam.
- The phone runs one camera at a time: a second request while one is on is refused as busy.
- A camera is never driven, and the sound that comes with one is the device's microphone,
  not what the device plays.
- A program that reads only V4L2 sees a presented camera only with `v4l2loopback` loaded.
