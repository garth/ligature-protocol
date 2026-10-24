# The clipboard

Copy on one device, paste on another. Text, formatted text, images and files all travel;
small content is pushed the moment you copy, large content is fetched only when you paste
it, and nothing is kept anywhere but on your devices.

## What travels

| Content | How |
| --- | --- |
| Text | Pushed to every device as you copy, up to 512 KiB |
| Formatted text (HTML) | Pushed beside the text, within the same bound |
| Images (PNG) | Pushed when small; announced when large, and fetched when an application pastes |
| Files and folders copied in a file manager | Always announced; fetched into a cache when pasted |

A device announces what it did not push and keeps the bytes ready for one hour, or until
you copy something else, whichever comes first. An announced image or file that is pasted
after that is gone, and the paste fails; the text that came with it stays. Up to 64 MiB of
announced content is kept per copy.

## Linux

Copying and pasting work as they always do; the daemon watches the Wayland clipboard and
sets it when another device copies. A file manager that pastes a copied file list gets the
files fetched into `$XDG_RUNTIME_DIR/ligature/clipboard` and pasted from there. Clipboard
history tools that read every selection as it appears (Omarchy's does) fetch a large image
the moment it is announced rather than when you paste.

## macOS

The same, through the pasteboard. macOS gives no event when the pasteboard changes, so
the daemon reads its change counter four times a second, one integer, and reads the
contents only when it has moved; Linux and Android tell the daemon of a change instead. A
copied file list arrives as items Finder can paste; the files are fetched into
`~/Library/Caches/org.ligatureprotocol.ligatured/clipboard` and Finder copies them from
there. TextEdit pastes the first type it can read of what arrived.

## Android

Receiving needs nothing: whatever you copy elsewhere is on the phone's clipboard while the
node runs. Text and formatted text go on the clip itself. An image or a file list arrives as
content the app serves, so pasting it in another app fetches the bytes from the device that
copied them. Because few Android apps paste files, a copied file list also raises a
notification whose **Save** puts the files in Downloads.

Sending is on your request, because Android lets an app read the clipboard only while it has
the focus. Any of these sends what is on the clipboard:

- the **Send clipboard** quick-settings tile,
- the **Send clipboard** action on the Ligature notification,
- **Send clipboard now** on the app's screen,
- opening the app,
- **Send to my devices** in the share sheet, which sends the shared text after showing it to
  you.

**Receive now** on the app's screen asks your other devices for what they hold, for the
case where the phone was off or out of reach when you copied.

On a ROM you built and signed yourself the app can hold the platform permission to read the
clipboard in the background, and then every copy is sent as it happens; the screen reports
"automatic (background read allowed)". An official LineageOS build cannot grant it. The
[app's README](https://github.com/garth/ligature-protocol/blob/main/impl/android/README.md)
has the signing steps.

## What stays where

The clipboard is never stored. A device keeps the current value in memory for its other
devices to ask for, and a device that connects later asks for it, so a laptop that wakes up
gets what you last copied on the phone. Fetched content is cached under the runtime
directory and cleared when the clipboard changes and when the daemon starts.

## From the command line

`lpctl clipboard` reads and sets the clipboard through the daemon, for checking that the
chain works:

```sh
lpctl clipboard get                 # print the clipboard's text
lpctl clipboard set "hello"         # put text on it, and so on every device
lpctl clipboard put image/png a.png # put one typed part on it, from a file
lpctl clipboard paste image/png out.png   # paste one type into a file, fetching it if needed
```
