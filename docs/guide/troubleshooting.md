# Troubleshooting

## The daemon is not running

`lpctl` says it cannot reach the daemon.

- Linux: `systemctl --user status ligatured`, and `journalctl --user -u ligatured -e` for
  the reason it stopped. `systemctl --user restart ligatured` starts it again.
- macOS: `launchctl print gui/$(id -u)/org.ligatureprotocol.ligatured | head` and
  `~/Library/Logs/ligature/ligatured.log`.
- Android: `Ligature is running` is the node's notification, but you may have swiped it away,
  so its absence proves nothing; the node's card on the app's screen shows the connected
  devices. Opening the app starts the node if it has stopped. Allow unrestricted battery use
  so Android does not stop it again, and note that **Force stop** in the app's settings keeps
  it stopped, even across a reboot, until the app is opened.

## Devices do not see each other

`lpctl status` shows no peers, or the phone's screen shows none connected.

1. **Same network.** Discovery uses mDNS, which does not cross routers; guest networks and
   client isolation on Wi-Fi block it. A device with a wired and a wireless connection
   announces on both.
2. **The firewall.** Peers dial each other on UDP; the services fix port 47471 so one rule
   suffices. If both devices block incoming UDP, neither can reach the other; opening the
   port on one of them is enough for the pair.
3. **Multicast on the phone.** The node holds the Wi-Fi multicast lock while it runs; a
   phone whose Wi-Fi was off when the node started finds nothing until the network comes
   back, which the app watches for.
4. **The device is revoked.** `lpctl devices` marks it `REVOKED`. It will never connect
   again; pair it as a new device.
5. **Different versions.** A device on an older release may be refused or misunderstood.
   Update every device.

The daemon logs each connection and each refusal with the peer's key at `info`;
`journalctl --user -u ligatured -f` while you bring a device up shows which step fails.

## Pairing fails

- The codes differ: another device on the network answered the offer. Say no and show a new
  offer.
- "a pairing is already in progress": a previous `lpctl pair` is still waiting; stop it.
- "already initialised": this device has keys; it can introduce but not join. For a fresh
  start, uninstall and delete the data directory.
- The introducer cannot reach the newcomer: the offer carries the newcomer's addresses, and
  the introducer dials them in turn. Check the firewall on the newcomer.

## The clipboard does not arrive

- **On Linux**, the compositor must speak `wlr-data-control`; the daemon withdraws the
  clipboard capability while it cannot reach one, and `lpctl caps` then lists no clipboard
  under "this device offers". A clipboard history tool that reads every selection fetches
  announced images at once, which is expected.
- **On the phone**, receiving is automatic. Sending is automatic while Ligature is on under
  Accessibility, and the clipboard card says "Copies on this phone go to your other devices as
  they happen" when it is; otherwise it needs a tap: the tile, the notification action, the app
  or the share sheet. Opening the app sends what is on the clipboard. A copy the copying app
  marks as sensitive is never sent by itself, and a copy made seconds after another arrives
  about seven seconds late ([The clipboard](clipboard.md#android)).
- **Large content** is fetched when pasted and only for an hour after the copy; a paste that
  fails with the text intact means the announcement has expired or the copying device is
  gone. Copy again.
- `lpctl caps` shows whether the peer offers the clipboard at all and with which types.

## Notifications do not arrive

- **On the Mac**, Ligature must be allowed under System Settings → Notifications, and the
  daemon must run from `~/Applications/Ligature.app`, as the installer sets it up; run from
  anywhere else it displays nothing, and the log says so. The Mac never sends its own.
- **On the phone**, grant notification access for the phone's notifications to leave it, and
  check the app's switch in the **Notifications** card.
- **On Linux**, a notification daemon must be running on the session bus for display; for
  sending, the daemon monitors the bus and needs nothing else.
- `lpctl notify "test"` from a desktop is the quickest check of the whole path.

## The keyboard and mouse stay with another device

- Press both Control keys together: the keyboard and mouse come back to this device.
- A desktop gives its keyboard and mouse back on its own when the pointer reaches an edge
  but the other device cannot be driven, for example because it is asleep or out of reach.
- If nothing responds, restart the daemon from another way in, such as a terminal on
  another device: the compositor ends the capture when the daemon's session goes.
  `systemctl --user restart ligatured` on Linux, or
  `launchctl kickstart -k gui/$(id -u)/org.ligatureprotocol.ligatured` on a Mac.

## The phone cannot be driven or watched

- The **Remote control** card on the app's screen says whether your devices can control the
  phone. When it asks you to turn Ligature on under Accessibility, the switch is off: it was
  never turned on, the app was reinstalled, or **Remove access** was tapped on Android's
  "Review app with full device access" reminder. Turn it on again
  ([Driving the phone](input.md#driving-the-phone)).
- A watch that never starts is waiting for someone to answer Android's dialog on the phone,
  for a minute, unless the grant in
  [Watching the phone with nobody at it](screen.md#watching-the-phone-with-nobody-at-it) is in
  place; the card says "watch its screen with nobody here" when it is.
- A watch that shows only the lock screen is a locked phone: its PIN screen is never shared.
- A dark screen that stays dark: Settings, Apps, Special app access, **Turn screen on** must
  allow Ligature.

## Input sharing does not start on the Mac

- `lpctl permissions` on the Mac lists which permissions the daemon holds. A missing one is
  turned on under System Settings, Privacy & Security (`lpctl permissions open
  accessibility`, or `input-monitoring`, opens the page); some macOS versions list the
  Accessibility switch under Device Control and Data Access. The daemon restarts by itself
  a few seconds after you allow one, so it takes effect without a restart of your own.
  `lpctl caps` on another device shows whether the Mac offers input.
- A switch that shows as on can belong to a previous version: macOS ties these permissions
  to the app's signature, which changes with every version. `lpctl permissions reset` clears
  them and restarts the daemon; allow each again when macOS asks
  ([Upgrading](upgrade-uninstall.md#upgrading)).
- The log says what the daemon sees: `grep -i "input capture" ~/Library/Logs/ligature/ligatured.log`.

## A file does not arrive

- The receiver decides where files land and how many at once; `lpctl files offers` prints
  this device's download folder. A phone takes one file at a time, so a second waits.
- A transfer that ends with `hash-mismatch` or `size-mismatch` was altered or cut short in
  flight; nothing is kept, and sending again is safe.
- `lpctl files transfers` on either end shows what is in flight and why one ended.

## Logs and reporting

Set `RUST_LOG=debug` on the daemon for the mechanism behind a problem: on Linux with
`systemctl --user edit ligatured` and an `Environment=RUST_LOG=debug` line, on the Mac by
editing the agent's plist, on the phone in `adb logcat -s ligature`. Logs name devices by
their keys and never carry what was copied, posted or sent. Report problems on the
[issue tracker](https://github.com/garth/ligature-protocol/issues); anything that looks like
a security weakness goes through a
[security advisory](https://github.com/garth/ligature-protocol/security/advisories/new).
