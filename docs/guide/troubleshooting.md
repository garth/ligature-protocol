# Troubleshooting

## The daemon is not running

`lpctl` says it cannot reach the daemon.

- Linux: `systemctl --user status ligatured`, and `journalctl --user -u ligatured -e` for
  the reason it stopped. `systemctl --user restart ligatured` starts it again.
- macOS: `launchctl print gui/$(id -u)/org.ligatureprotocol.ligatured | head` and
  `~/Library/Logs/ligature/ligatured.log`.
- Android: the persistent "Ligature is running" notification is the node; without it the
  service is stopped. Open the app to start it, and allow unrestricted battery use so
  Android does not stop it again.

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
- **On the phone**, receiving is automatic but sending needs a tap: the tile, the
  notification action, the app or the share sheet. Opening the app sends what is on the
  clipboard.
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

## Input sharing does not start on the Mac

- `lpctl caps` on another device shows whether the Mac offers input. When it offers none,
  Ligature is not allowed under Accessibility and Input Monitoring in System Settings,
  Privacy & Security; some macOS versions list the Accessibility switch under Device Control
  and Data Access.
- A switch that shows as on can belong to a previous version: macOS ties these permissions
  to the app's signature, which changes with every version. Clear them as
  [Upgrading](upgrade-uninstall.md#upgrading) shows and allow them again.
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
