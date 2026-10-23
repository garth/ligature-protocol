# First steps

Ligature knows you by a key, not an account. Your first device creates it; every other
device joins by pairing with a device that already has it. Once a device is paired, all your
devices know it: you never pair the same device twice.

## Create your keys on the first device

On a desktop, once the daemon is running:

```sh
lpctl init --name "Your Name"
```

The name is what your other devices and, later, other people see; it is yours to choose.
The device takes the machine's host name as its own; `--device-name` picks another.

On a phone, open Ligature, enter your name under **Or start here** and tap **Create my keys
on this phone**.

The first device becomes an *introducer*: it holds the key that signs new devices in, so it
is the device you pair the others with. A device paired later becomes an introducer too,
unless you say otherwise, so any of your devices can then add the next one.

## Pair another device

Pairing takes two devices in the same place: the *newcomer*, which has no keys yet, and an
*introducer*, one of your devices that has them. Both must be on the same network and
running.

1. **On the newcomer, show an offer.** On a desktop:

   ```sh
   lpctl pair
   ```

   It prints a QR code and the offer as text, `ligature:pair/1?…`, and waits. On a phone,
   tap **Show pairing offer**: the QR code and the text appear, with **Copy** and **Share**.

2. **On the introducer, use the offer.** On a desktop, paste the text:

   ```sh
   lpctl pair 'ligature:pair/1?…'
   ```

   On a phone, paste it under **Add another device** and tap **Introduce**. A desktop can
   also scan the phone's QR code with anything that reads one and paste the result. The
   offer names the newcomer's key and addresses on the network and nothing else; it is
   useless to anyone who is not on your network at that moment.

3. **Compare the code.** Both devices show the same six-digit code, derived from the
   connection they have just made. Check they match, then confirm on both: answer `y` at the
   prompt on a desktop, tap **Yes, same code** on a phone. If the codes differ, someone else
   answered the offer: say no, and nothing is signed.

4. **Done.** The introducer signs the newcomer in and hands it everything it knows about
   your devices; the newcomer connects to each of them as they appear. `lpctl devices` and
   the phone's **Devices** card list it on every device within a moment.

### Options

- `lpctl pair --no-introducer 'ligature:pair/1?…'` signs the newcomer in without the
  introducer's signing key, so it can use your devices but never add one. Use it for a
  device you trust less.
- `--yes` skips the code prompt on a desktop, for scripts. Compare the code some other way
  when you use it.
- A second pairing cannot start while one is waiting, and a device that already has keys
  cannot show an offer; stop the daemon's pairing with Ctrl-C (desktop) or **Cancel**
  (phone) first.

## Knowing other people

The protocol defines pairing with another person's device too, so that a household can
share between its members. The apps do not offer it yet: today every device you pair is
your own, and a device that is not yours is kept at the door.

## What happens underneath

Each device keeps its own record of your devices as signed facts and exchanges them with
every peer at the start of each session, revocations first. There is nothing to back up or
sync: a device that was offline learns what it missed from the next device it meets. The
[specification](../spec/00-overview.md) has the detail.
