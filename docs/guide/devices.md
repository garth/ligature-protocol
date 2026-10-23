# Your devices

## Seeing them

```sh
lpctl status
```

shows this device's person and keys, the port it listens on and the peers it is connected
to right now:

```text
ligatured 0.1.1-alpha
initialised: true
person:      Garth (z6Mkkaz8…)
device:      tmarchy (mCgtFQcD…)
binding:     z6MkmCgtFQcD…
principal:   z6MkmE9eX239…
port:        47471
peers:       1
  Pixel 8 Pro          android  pJSu7fVZ…  z6MkpJSu7fVZ…
```

```sh
lpctl devices
```

lists every device of yours this one knows, connected or not:

```text
Pixel 8 Pro          android   [introducer, connected]
  binding   pJSu7fVZ…  z6MkpJSu7fVZ…
  principal mQ8DV81x…  z6MkmQ8DV81x…
Garths-MacBook-Pro   macos     [introducer]
  binding   tXkoADza…  z6MktXkoADza…
  principal gsg6UFXj…  z6Mkgsg6UFXj…
tmarchy              linux     [this device, introducer]
  …
```

Every device has two keys: the *binding* is your account on it, the *principal* is the
device itself. Today both belong to you; the split is what will let a shared device such as
a family tablet serve several people. *Introducer* marks a device that can add others. The
phone shows the same list in its **Devices** card.

## Names

A device's name is its host name on the desktops (the name under About phone, else the
model, on Android), taken when the daemon first starts; `ligatured --device-name` on first
start or `lpctl init --device-name` chooses another. The name is what the others show; the
key is what identifies the device.

## Revoking a device

If a device is lost, stolen or simply retired, revoke it from any of your introducers:

```sh
lpctl revoke z6MkioscM4pN…
```

or **Revoke** beside it on the phone. The key is the binding or the principal as `lpctl
devices` lists them; either works. The revocation reaches your other devices at once if they
are connected and at their next session otherwise, and every device closes any session it
holds with the revoked one and refuses it from then on. The device stays in the list marked
`REVOKED`, so you can see what was cut off; nothing undoes a revocation. The revoked device
itself cannot announce or reverse anything: it simply finds no device that will talk to it.

Revoking a device you are still using is possible and final. Pair it again afterwards, with
a fresh install, as a new device.

## If you lose every introducer

There is nothing to recover and no one to ask: your identity lived on your devices alone.
Start over on a fresh install with `lpctl init` or **Create my keys on this phone**, pair
the devices you still hold, and revoke nothing, since the old key simply disappears. Other
people who knew the old key would re-verify you in person; today no one does, since the apps
pair only your own devices.

## Inspecting what devices offer

```sh
lpctl caps
```

shows the capabilities each connected device currently offers (clipboard, notifications,
files) with their limits, such as the clipboard's push bound and how many files a device
takes at once. The list is live: a device that disconnects drops out.

```sh
lpctl grants
```

lists the grants this device has issued. Between your own devices a grant is implicit and
recorded the first time a device uses a capability, so the list is a record of what has
been used, not something you manage. Grants never leave the device that made them.
