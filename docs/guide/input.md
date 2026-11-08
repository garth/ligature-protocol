# Keyboard and mouse

One keyboard and mouse can drive all your desktops. Push the pointer off an edge of one
screen and it carries on onto the device you put beside that edge, typing and clicking
there until it comes back the way it went. The phone can also be a touchpad and keyboard for
any of your desktops, and can be driven itself once you allow it.

## Placing your devices

Each desktop records which of its own edges leads to which device. For a Linux machine with
the Mac to its right:

```sh
lpctl input layout set right Garths-MacBook-Pro   # on the Linux machine
lpctl input layout set left tmarchy               # on the Mac
```

The device is named as `lpctl devices` shows it, by its display name or a prefix of its key.
The pointer comes back through the edge it went in by, so a device needs its edges set only
to drive others. A desktop that only ever gets driven needs none.

```sh
lpctl input layout             # which edges lead where
lpctl input layout unset right # an edge that leads nowhere
```

## Driving another desktop

Move the pointer off the edge. It appears on the other device at the matching height, and
your keyboard, buttons and scroll wheel go there with it. Move it back off the edge it came
in by and it returns to this screen at the height where it left.

To take the keyboard and mouse back at any moment, press **both Control keys** together, or
**both Alt keys** (both Option keys on a Mac, whose keyboard has a Control key on one side
only), or run `lpctl input release` from another terminal. A desktop also gives them back by
itself when the device beside the edge is asleep, out of reach or refuses, and when the driven
device's daemon stops in the middle of a drive. Nothing stays pressed on the other side: every
key and button held there is let go when the drive ends.

```sh
lpctl input status   # what this device offers, drives and is driven by, with the round trip
```

## What your keys type

A key arrives as what it means on the keyboard you are typing on, not as a position. On the
device you drive, you get what your own keycap and layout say, whatever layout that device
uses: `@` on a British Mac is `@` on a Linux machine set to US. Shortcuts keep their letter,
so Control and C is Control and C there. Modifiers keep their meaning wherever the other
layout puts them.

- On a Mac, **Command is Super** and **Option is Alt**. A character you would type with
  Option on the Mac is not sent as that character: the other device sees Alt with the key,
  since Option is its Alt key.
- **Caps Lock** turns the other device's Caps Lock on and off.
- An accent key (a dead key) is combined on your own device first. The other device receives
  the finished character, such as `é`.
- A character the other device's layout has no key for is still typed.

## The phone as a touchpad

On the app's main screen, under **Touchpad**, tap **Drive** followed by the desktop's name
(**Drive tmarchy**, say). Then:

- one finger moves the pointer, a tap clicks, and a touch held still then moved drags;
- two fingers scroll;
- the field types into the desktop, whatever its layout, and **Backspace**, **Enter** and
  **Right click** are buttons of their own;
- **Done**, or leaving the screen, ends the drive.

A desktop that the phone is driving still follows its own layout, so the phone can push the
pointer on across an edge to the next desktop.

## Driving the phone

Android lets an app act on the screen only as an accessibility service, so the phone can be
driven only once you turn that on. On the app's main screen, under **Remote control**, tap
**Accessibility settings** and turn on Ligature. If Android says the setting is restricted,
which it does for an app you installed yourself, open the app's page in Settings, choose
**Allow restricted settings** from its menu, and try again. Turn it off the same way to stop
your devices driving the phone. Ligature cannot read what is on the screen through this. The
same switch sends what you copy on the phone to your devices ([The clipboard](clipboard.md#android)).

Android reminds you of an app with this much access: Settings, Security and privacy may show
**Review app with full device access** naming Ligature, and its **Remove access** turns the
switch off. Uninstalling the app turns it off too, so turn it on again after a reinstall; an
update keeps it.

A drive or a watch turns the phone's screen on and keeps it on while it lasts, since a dark
screen takes no touch and shows nothing. Android lists this under Settings, Apps, Special app
access, **Turn screen on**. The lock screen stays: whoever drives the phone gets past it as
anyone at the phone would, dragging up to the PIN pad and clicking the PIN.

Then the phone is a device you can drive, from `lpview` or across an edge like any other:

- a dot on the screen is the pointer, and the left button is a finger at it, so a click is a
  tap and a held button drags;
- the right button is Back and the middle button is Home; Escape is Back as well;
- the wheel scrolls, as a swipe;
- typing goes to the text field in focus, so click into a field first, and Control or Command
  shortcuts such as select all work there. What you copy or cut with them reaches your
  devices' clipboards straight away, and paste pastes what you last copied on any of them.

Android sets some limits on this. Keys reach a text field and nothing else, so an app's own
keyboard shortcuts outside a field do nothing, and typing needs Android 13. Holding a key does
not repeat it. If you touch the screen during a drag, Android cancels the drag.

## What each platform needs

- **Linux, to drive others**: a desktop portal that offers InputCapture, which Hyprland's
  (`xdg-desktop-portal-hyprland` 1.4.1) does. Driving others has been tried on Hyprland only.
- **Linux, to be driven**: a compositor with the virtual pointer and virtual keyboard
  protocols, such as Hyprland or Sway. GNOME and KDE Plasma do not offer a virtual pointer to
  applications, so they cannot be driven.
- **Mac**: allow Ligature under Accessibility to be driven, and under Input Monitoring as well
  to drive others ([Install](install.md#macos)). Until then the Mac offers neither.

## Limits

- Input goes between your own devices only.
- The keyboard and mouse follow the pointer. There is no shortcut that jumps to a device
  without crossing an edge, except the phone's touchpad and watching a screen.
- Each desktop's layout is set on that desktop. Nothing yet sets the whole arrangement from
  one place.
