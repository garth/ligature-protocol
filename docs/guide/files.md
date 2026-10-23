# Files

A file chosen on one device lands in another's download folder over the same connection
everything else uses, with progress and a cancel at either end. A device can also offer a
file for another to fetch, which is how the clipboard carries large content.

## Sending from a desktop

```sh
lpctl files send phone ~/Pictures/holiday.jpg ~/Documents/notes.pdf
```

The device is named by its display name, or by a prefix of its key or fingerprint as
`lpctl devices` shows them. Several files go in one transfer; the receiver decides how many
move at once (a phone takes one at a time, a desktop four), so a small file never waits
behind a large one for long. The command prints the transfer id and returns; the transfer
runs in the daemon.

```sh
lpctl files transfers          # what is in flight, with progress
lpctl files cancel <transfer>  # stop one; nothing partial is left on either side
```

## Sending from the phone

Share one or several files to **Send to my devices** and pick the device that should receive
them. A notification shows the transfer with a progress bar and **Cancel**, and becomes
**Sent** at the end. Only files other apps hand over through the share sheet can be sent;
the app never opens paths of its own.

## Receiving

Nothing to do. A received file lands in the download folder (`XDG_DOWNLOAD_DIR` or
`~/Downloads` on Linux, `~/Downloads` on the Mac, Downloads on the phone) under its own
name; a name already taken gets a number. The file is written to a temporary name and
renamed into place only after its hash has been checked, so a half-received file never looks
like a whole one. Linux announces a received file with a notification; the phone's transfer
notification becomes **Received**, and tapping it opens the file.

## Offering and fetching

Instead of pushing, a device can offer a file and let another fetch it:

```sh
lpctl files offer ~/big.iso        # prints the offer's id
lpctl files offers                 # what this device offers
lpctl files withdraw <id>
```

and on the other device:

```sh
lpctl files get laptop <id>              # into the download folder
lpctl files get laptop <id> --to /mnt/x  # or somewhere else
```

Offers live in memory: they end when the daemon stops or the connection drops. A fetch of a
withdrawn id is refused.

## Limits

- One connection per pair of devices carries everything, so a transfer shares the link with
  the clipboard and notifications; chunks are small enough that neither stalls the other.
- Files travel between your own devices only. Sharing with other people is specified but not
  built.
- There is no resume: a transfer that drops is started again from the beginning.
