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
person:      Garth (z6Mkkaz8L7J2HSv4q15GT7vTH9Pw8PfacYetAanAS8bZfGC5)
device:      tmarchy (mCgtFQcD…)
binding:     z6MkmCgtFQcDLypvVgsGKV3C5k1EfrpXWTByDcCdW5UyQUmj
principal:   z6MkmE9eX239nwPy7jAouMGTLQ1oEbAQKbTfZ334841xYiJt
port:        47471
peers:       2
  Garths-MacBook-Pro   macos    tXkoADza…  z6MktXkoADzaQgSWxx6t4GMKmdB6GyBAe6vvyaLmYFpatQqU
  Pixel 8 Pro          android  pJSu7fVZ…  z6MkpJSu7fVZgXVWvT5fHZYukXzfp73dRqnFcjJPbicmKBRD

  lpctl devices  lists every device with its keys
  lpctl pair     adds a device: run it on the new device and give the offer it shows to lpctl pair <OFFER> here
```

```sh
lpctl devices
```

lists every device of yours this one knows, connected or not:

```text
Pixel 8 Pro          android   [introducer, connected]
  binding   pJSu7fVZ…  z6MkpJSu7fVZgXVWvT5fHZYukXzfp73dRqnFcjJPbicmKBRD
  principal mQ8DV81x…  z6MkmQ8DV81x4qw5DzvyiKaG42yfDChrq38A4BAWxNZg7imf
Pixel 8 Pro          android   [introducer, REVOKED]
  binding   ioscM4pN…  z6MkioscM4pNAV5wJue9SYC9sH7MTtyvjrT1RKHFVzCRYuFx  REVOKED
  principal izDBk2qS…  z6MkizDBk2qS7Whc8ofkoLbesBeJLZeHTXrc7EzRn5JdNZJc  REVOKED
Garths-MacBook-Pro   macos     [introducer, connected]
  binding   tXkoADza…  z6MktXkoADzaQgSWxx6t4GMKmdB6GyBAe6vvyaLmYFpatQqU
  principal gsg6UFXj…  z6Mkgsg6UFXjzkTCTdLuH8m7ZpL8EXoyPsCnyzgxZGGYQ8eM
tmarchy              linux     [this device, introducer]
  binding   mCgtFQcD…  z6MkmCgtFQcDLypvVgsGKV3C5k1EfrpXWTByDcCdW5UyQUmj
  principal mE9eX239…  z6MkmE9eX239nwPy7jAouMGTLQ1oEbAQKbTfZ334841xYiJt

  lpctl pair          adds a device: run it on the new device and give the offer it shows to lpctl pair <OFFER> here
  lpctl revoke <KEY>  revokes a device's or person's key, on every device
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

or **Revoke** beside it on the phone. The key is the whole `z6Mk…` key of the binding or the
principal, as the last column of `lpctl devices` shows it; a name or a prefix is not
accepted, and either key works. The revocation reaches your other devices at once if they
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

shows the capabilities this device and each connected device currently offer (clipboard,
notifications, files, keyboard and mouse, screen) with their limits, such as the clipboard's push
bound and how many files a device
takes at once. The list is live: a device that disconnects drops out.

```sh
lpctl grants
```

lists the grants this device has issued. Between your own devices a grant is implicit and
recorded the first time a device uses a capability, so the list is a record of what has
been used, not something you manage. Grants never leave the device that made them.
