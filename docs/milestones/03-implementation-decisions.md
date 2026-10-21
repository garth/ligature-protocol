# Milestone 3 implementation decisions

Choices that are not part of the protocol but that carrying notifications needs settled.
Each is recorded under the ticket that made it; the protocol-level choices are in
[03-notifications.md](03-notifications.md) section 3.

## LP-91: the daemon core and Linux

**One synchroniser per daemon.** `ligatured::notification_sync::NotificationSync` sits
beside `ClipboardSync` and follows its shape: a stream handler registered for the
capability type, one outgoing stream per same-person peer opened when the peer connects, a
watcher on the local backend's events, and an advertiser that offers the descriptor with the
directions the backend reports and re-offers it when they change (`advertise`, called by a
host whose notification access was granted or revoked). Everything it holds, the icon
caches and the counters, lives in memory for the session.

**The backend trait.** `ligatured::notifications::Notifications` is what a platform
supplies: whether it emits and displays and how large an icon it takes; `show`,
`dismiss_copy` and `dismiss_own`; and a broadcast of local events (`Posted`, `Removed`,
`CopyDismissed`). The trait is synchronous like `Clipboard`; the Linux backend answers from
an async task through a channel. `FakeNotifications` serves the tests and the loopback run
(`LIGATURE_FAKE_NOTIFICATIONS=1`).

**Message types in `lp-core`.** `NotificationMessage` is an internally tagged enum on
`type`, so one `recv` on the stream yields a post, an icon or a dismiss; the icon's bytes
are a CBOR byte string through `wire::Bytes`, and its hash is SHA-256 in base64url without
padding, checked on receipt. `NotificationsParams::from_descriptor` reads a peer's
directions from its manifest with the schema's defaults for missing fields.

**Icons go once per peer per session**, keyed by hash in `icons_sent`; a peer whose
`maxIconBytes` is smaller than the icon gets the post without it. Received icons are
cached per peer and hash; both caches go when the peer disconnects.

**`lpctl notify`** posts through the same path as an app would, under the app id
`org.ligatureprotocol.lpctl`, and goes out even on a device that does not emit, since the
user asked for it by hand. It is the diagnostic for "does a notification from here reach
there" and nothing more.

**Linux over the session bus with `zbus` 6.** The daemon finds the bus at
`$DBUS_SESSION_BUS_ADDRESS`, else `$XDG_RUNTIME_DIR/bus`, the way it finds the compositor
socket, so it needs no imported environment. Two connections: one calls `Notify` and
`CloseNotification` on `org.freedesktop.Notifications`; the other becomes a monitor
(`org.freedesktop.DBus.Monitoring.BecomeMonitor`) with rules for `Notify` calls, every
method return (the reply carries the id a call was given) and `NotificationClosed`. A
monitor may only receive, hence the second connection. Checked on the owner's machine
(dbus-broker, the quickshell notification daemon): the monitor is granted to the session's
own user, a `notify-send` is seen with its id, and a copy shown with the daemon's own call is
not seen as a post, since the call carries the `x-ligature-origin` hint. `zbus` 6 rather
than 5 because cargo-deny reports the 5 line's support crates as unmaintained; it needs
Rust 1.89, so the workspace's minimum rose by one minor version.

**What Linux emits.** A `Notify` without the origin hint and without the `transient` hint,
once its reply names the id; the id becomes the notification's id as a string, and a
`replaces_id` keeps the id of the notification it replaces. The app id and name are both
the `app_name` the caller gave, since the bus carries nothing better. The icon travels
only when `app_icon` is an absolute path or `file://` URI to a PNG within the bound; icon
names from a theme are not resolved in this milestone. A `NotificationClosed` for an
emitted notification with reason 2 (dismissed by the user) or 3 (closed by the app)
becomes a `Removed`; reason 1 (expired) does not, since a timeout here should not clear
the phone's.

**What Linux displays.** `Notify` with the app name as "App · Device", the origin hint, a
`desktop-entry` hint carrying the app id, no actions and the default timeout; the icon is
written once under `$XDG_RUNTIME_DIR/ligature/icons/<hash>.png` and passed as the icon
path. The bus id of every copy is kept per origin and id so a repeated post replaces and a
dismiss closes; the map is shared with the monitor, so a `NotificationClosed` with reason
2 for one of our copies becomes a `CopyDismissed` naming the origin and id it was shown
for.
