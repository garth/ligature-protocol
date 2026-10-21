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

## LP-92: Android

**The listener is the emitter.** `NotificationListener` is a `NotificationListenerService`
the user enables under notification access; `emits` in the phone's descriptor follows
whether it is connected (`Node::set_notifications_emitting`), so the desktops see the
direction appear and disappear rather than a silent stop. The `Host` trait gained
`show_notification`, `dismiss_notification_copy` and `dismiss_own_notification`, and the
node gained `notification_posted`, `notification_removed` and
`notification_copy_dismissed`; `HostNotifications` is the backend behind them, as
`HostClipboard` is for the clipboard.

**What leaves the phone.** A posted notification with a title or text, unless it is
ongoing, a foreground service's, a group summary, below default importance in the
ranking, Ligature's own, or from an app the user switched off. Its id is the listener's
key, which is stable for the notification's life and what `cancelNotification` wants
back. The icon is the app's launcher icon rendered to a 96-pixel PNG, cached per app,
since small icons are monochrome masks that look wrong on a desktop; anything over the
64 KiB bound is sent without an icon.

**Removals.** A removal reaches the other devices when the user dismissed or tapped the
notification or the app cancelled it (`REASON_CLICK`, `REASON_CANCEL`,
`REASON_CANCEL_ALL`, `REASON_APP_CANCEL`, `REASON_APP_CANCEL_ALL`); a cancel the listener
made itself on another device's behalf (`REASON_LISTENER_CANCEL`) does not, which is the
loop rule on this platform, and a timeout clears nothing elsewhere.

**Copies.** Shown through the notification manager in a channel per origin device, named
after it, with the app name and the device in the sub-text line, the app's icon as the
large icon, the origin's key as the tag and the id's hash as the number, so a repeat
replaces and a dismiss cancels. The origin and id ride in the notification's extras, and
the listener reads them back when a copy is removed by the user's hand to send the
dismissal on.

**Per-app switches.** The app list is the set of packages the listener has seen post,
kept in the app's preferences with their labels, each on unless switched off; the
exclusion set is checked at the listener. Nothing else is configurable yet (LP-88).

## LP-98: one connection per pair, agreed by both ends

**Found in the run.** With the reconnect and discovery loops of LP-83 on every device, a
pair often holds two QUIC connections for a moment, and each end used to settle the
duplicate on its own ("keep the live one"), so a restarted or silently lost peer was
refused as a duplicate of a connection the other end still held, for up to the idle
timeout. The Mac's first dial after its restart was closed by the phone that way.

**Decision.** `lp-transport::Manager` applies one rule at both ends: when the same side
dials again, the new connection wins (a completed dial is proof the peer is there now,
and the old one may have died without a word); when the dials crossed, the one dialled
by the lower binding key wins. The dialler and the acceptor compute the same answer, so
both close the same connection. Discovery no longer dials an instance whose key is
already connected, which removes most repeat dials at the source.

**Also found.** Two capabilities offered at the same moment could lose each other's
entry, since the registry read, changed and wrote its list without holding anything
across the change; the phone's manifest then lacked the clipboard one run in three.
`Registry` now holds a lock across a change and the manifests it sends, so offers and
withdrawals are serialised and never arrive out of order.
