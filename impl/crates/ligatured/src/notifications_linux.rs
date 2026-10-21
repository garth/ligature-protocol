//! Notifications on Linux over the session bus. Spec chapter 05 section 3c
//! and chapter 08 section 2; milestone 3 (LP-91).
//!
//! Display: `Notify` on `org.freedesktop.Notifications` with the origin device
//! named beside the app and an `x-ligature-origin` hint; dismiss with
//! `CloseNotification`. Emit: a second connection becomes a monitor of the
//! bus and sees every `Notify` any app makes, the reply that assigns its id,
//! and every `NotificationClosed`, whatever notification daemon is in use.
//! Our own calls carry the hint and are skipped, as are `transient` ones.
//! The bus is found at `$DBUS_SESSION_BUS_ADDRESS`, else
//! `$XDG_RUNTIME_DIR/bus`, the way the compositor socket is found.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt as _;
use lp_core::wire::Timestamp;
use lp_core::{Error, PublicKey};
use tokio::sync::{broadcast, mpsc, oneshot};
use zbus::{OwnedVariant, Variant};

use crate::notifications::{Event, Icon, Notification, Notifications};

/// The hint our own `Notify` calls carry, so the monitor never forwards them.
const ORIGIN_HINT: &str = "x-ligature-origin";

/// How many local events are buffered for the synchroniser.
const EVENT_BUFFER: usize = 64;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait FdoNotifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Variant<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;
}

/// What the display task is asked to do.
enum Command {
    Show {
        origin: PublicKey,
        origin_name: String,
        notification: Box<Notification>,
        done: oneshot::Sender<Result<(), Error>>,
    },
    DismissCopy {
        origin: PublicKey,
        id: String,
        done: oneshot::Sender<Result<(), Error>>,
    },
    DismissOwn {
        id: String,
        done: oneshot::Sender<Result<(), Error>>,
    },
}

/// The Linux backend.
pub struct LinuxNotifications {
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<Event>,
    emitting: Mutex<bool>,
    /// Our copies: (origin, id) -> the bus id, so a repeat replaces, a
    /// dismiss closes, and the monitor knows which copy the user closed.
    copies: Mutex<HashMap<(PublicKey, String), u32>>,
}

impl std::fmt::Debug for LinuxNotifications {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LinuxNotifications")
    }
}

/// The session bus address, from the environment or the runtime directory.
fn bus_address() -> Option<String> {
    if let Some(addr) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS") {
        return Some(addr.to_string_lossy().into_owned());
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let path = std::path::Path::new(&runtime).join("bus");
    path.exists()
        .then(|| format!("unix:path={}", path.display()))
}

impl LinuxNotifications {
    /// Starts the backend on the session bus, if one can be found. Display
    /// works as soon as the bus answers; emitting starts once the monitor is
    /// up, and `emits` reports whether it is.
    #[must_use]
    pub fn start() -> Option<Arc<Self>> {
        let address = bus_address()?;
        let (commands, rx) = mpsc::channel(EVENT_BUFFER);
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let this = Arc::new(Self {
            commands,
            events,
            emitting: Mutex::new(false),
            copies: Mutex::new(HashMap::new()),
        });
        let displayer = this.clone();
        let addr = address.clone();
        tokio::spawn(async move { displayer.display_loop(addr, rx).await });
        let monitor = this.clone();
        tokio::spawn(async move { monitor.monitor_loop(address).await });
        Some(this)
    }

    async fn display_loop(self: Arc<Self>, address: String, mut rx: mpsc::Receiver<Command>) {
        let conn = match connect(&address).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(%e, "cannot reach the session bus; notifications not displayed");
                return;
            }
        };
        let proxy = match FdoNotificationsProxy::new(&conn).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(%e, "no notification service on the session bus");
                return;
            }
        };
        let icons = icon_dir();
        while let Some(command) = rx.recv().await {
            match command {
                Command::Show {
                    origin,
                    origin_name,
                    notification,
                    done,
                } => {
                    let replaces = self
                        .lock_copies()
                        .get(&(origin, notification.id.clone()))
                        .copied()
                        .unwrap_or(0);
                    let app_icon = notification
                        .icon
                        .as_ref()
                        .and_then(|i| icons.as_ref().and_then(|d| write_icon(d, i)))
                        .unwrap_or_default();
                    let mut hints: HashMap<&str, Variant<'_>> = HashMap::new();
                    hints.insert(ORIGIN_HINT, Variant::from(origin.to_string()));
                    hints.insert("desktop-entry", Variant::from(notification.app_id.clone()));
                    let result = proxy
                        .notify(
                            &format!("{} · {}", notification.app_name, origin_name),
                            replaces,
                            &app_icon,
                            notification.title.as_deref().unwrap_or_default(),
                            notification.body.as_deref().unwrap_or_default(),
                            &[],
                            hints,
                            -1,
                        )
                        .await;
                    let outcome = match result {
                        Ok(id) => {
                            self.lock_copies()
                                .insert((origin, notification.id.clone()), id);
                            Ok(())
                        }
                        Err(e) => Err(Error::Io(format!("notify: {e}"))),
                    };
                    let _ = done.send(outcome);
                }
                Command::DismissCopy { origin, id, done } => {
                    let removed = self.lock_copies().remove(&(origin, id));
                    let outcome = match removed {
                        Some(bus_id) => proxy
                            .close_notification(bus_id)
                            .await
                            .map_err(|e| Error::Io(format!("close: {e}"))),
                        None => Ok(()),
                    };
                    let _ = done.send(outcome);
                }
                Command::DismissOwn { id, done } => {
                    let outcome = match id.parse::<u32>() {
                        Ok(bus_id) => proxy
                            .close_notification(bus_id)
                            .await
                            .map_err(|e| Error::Io(format!("close: {e}"))),
                        Err(_) => Ok(()),
                    };
                    let _ = done.send(outcome);
                }
            }
        }
    }

    /// Watches every notification on the bus. A `Notify` call is remembered
    /// by its serial until the reply assigns the id, then published as
    /// posted unless it is ours or transient; `NotificationClosed` publishes
    /// a removal of an app's notification, or a copy's dismissal when the
    /// user closed one of ours.
    async fn monitor_loop(self: Arc<Self>, address: String) {
        let conn = match connect(&address).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(%e, "cannot reach the session bus; notifications not emitted");
                return;
            }
        };
        let rules = match monitor_rules() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%e, "bad monitor rules");
                return;
            }
        };
        let monitoring = match zbus::fdo::MonitoringProxy::new(&conn).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(%e, "no monitoring interface on the bus");
                return;
            }
        };
        if let Err(e) = monitoring.become_monitor(&rules, 0).await {
            tracing::warn!(%e, "the bus refused monitoring; notifications not emitted");
            return;
        }
        *self.lock_emitting() = true;
        tracing::info!("monitoring the session bus for notifications");
        let mut stream = zbus::MessageStream::from(&conn);
        // Notify calls seen, by serial, until their reply names the id.
        let mut pending: HashMap<u32, Pending> = HashMap::new();
        // The id of every notification we have told peers about, with what
        // we know of it; and the bus ids of our own calls, which are never
        // posts.
        let mut emitted: HashMap<u32, Pending> = HashMap::new();
        let mut ours: HashMap<u32, PublicKey> = HashMap::new();
        while let Some(message) = stream.next().await {
            let Ok(message) = message else { continue };
            let header = message.header();
            match header.message_type() {
                zbus::message::Type::MethodCall => {
                    if header
                        .interface()
                        .is_some_and(|i| i.as_str() == "org.freedesktop.Notifications")
                        && header.member().is_some_and(|m| m.as_str() == "Notify")
                        && let Ok(call) = message.body().deserialize::<NotifyArgs>()
                    {
                        pending.insert(header.primary().serial_num().get(), Pending::from(call));
                    }
                }
                zbus::message::Type::MethodReturn => {
                    if let Some(reply_to) = header.reply_serial()
                        && let Some(call) = pending.remove(&reply_to.get())
                        && let Ok(id) = message.body().deserialize::<u32>()
                    {
                        if let Some(origin) = &call.origin {
                            if let Ok(key) = origin.parse::<PublicKey>() {
                                ours.insert(id, key);
                            }
                            continue;
                        }
                        if call.transient {
                            continue;
                        }
                        let mut notification = call.to_notification(id);
                        if call.replaces_id != 0 {
                            // An update keeps the id the app replaced.
                            notification.id = call.replaces_id.to_string();
                            emitted.remove(&id);
                        }
                        let _ = self.events.send(Event::Posted(notification));
                        emitted.insert(id, call);
                    }
                }
                zbus::message::Type::Signal => {
                    if header
                        .interface()
                        .is_some_and(|i| i.as_str() == "org.freedesktop.Notifications")
                        && header
                            .member()
                            .is_some_and(|m| m.as_str() == "NotificationClosed")
                        && let Ok((id, reason)) = message.body().deserialize::<(u32, u32)>()
                    {
                        if ours.remove(&id).is_some() {
                            // One of our copies closed; 2 means the user did it.
                            let copy = {
                                let mut copies = self.lock_copies();
                                let key = copies
                                    .iter()
                                    .find(|(_, bus_id)| **bus_id == id)
                                    .map(|(k, _)| k.clone());
                                key.and_then(|k| copies.remove_entry(&k)).map(|(k, _)| k)
                            };
                            if reason == 2
                                && let Some((origin, copy_id)) = copy
                            {
                                let _ = self.events.send(Event::CopyDismissed {
                                    origin,
                                    id: copy_id,
                                });
                            }
                            continue;
                        }
                        if emitted.remove(&id).is_some() && (reason == 2 || reason == 3) {
                            let _ = self.events.send(Event::Removed { id: id.to_string() });
                        }
                    }
                }
                _ => {}
            }
        }
        *self.lock_emitting() = false;
        tracing::warn!("the session bus monitor ended; notifications no longer emitted");
    }

    fn lock_copies(&self) -> std::sync::MutexGuard<'_, HashMap<(PublicKey, String), u32>> {
        self.copies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_emitting(&self) -> std::sync::MutexGuard<'_, bool> {
        self.emitting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ask(
        &self,
        command: Command,
        done: oneshot::Receiver<Result<(), Error>>,
    ) -> Result<(), Error> {
        self.commands
            .try_send(command)
            .map_err(|_| Error::Io("notification display is not running".to_owned()))?;
        // The caller is synchronous; the display task answers quickly.
        tokio::task::block_in_place(|| done.blocking_recv())
            .unwrap_or_else(|_| Err(Error::Io("notification display went away".to_owned())))
    }
}

impl Notifications for LinuxNotifications {
    fn emits(&self) -> bool {
        *self.lock_emitting()
    }

    fn displays(&self) -> bool {
        true
    }

    fn max_icon_bytes(&self) -> u64 {
        lp_core::capability::NotificationsParams::DEFAULT_MAX_ICON_BYTES
    }

    fn show(
        &self,
        origin: &PublicKey,
        origin_name: &str,
        notification: &Notification,
    ) -> Result<(), Error> {
        let (done, rx) = oneshot::channel();
        self.ask(
            Command::Show {
                origin: *origin,
                origin_name: origin_name.to_owned(),
                notification: Box::new(notification.clone()),
                done,
            },
            rx,
        )
    }

    fn dismiss_copy(&self, origin: &PublicKey, id: &str) -> Result<(), Error> {
        let (done, rx) = oneshot::channel();
        self.ask(
            Command::DismissCopy {
                origin: *origin,
                id: id.to_owned(),
                done,
            },
            rx,
        )
    }

    fn dismiss_own(&self, id: &str) -> Result<(), Error> {
        let (done, rx) = oneshot::channel();
        self.ask(
            Command::DismissOwn {
                id: id.to_owned(),
                done,
            },
            rx,
        )
    }

    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}

async fn connect(address: &str) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::address(address).build().await
}

fn monitor_rules() -> zbus::Result<Vec<zbus::MatchRule<'static>>> {
    Ok(vec![
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::MethodCall)
            .interface("org.freedesktop.Notifications")
            .member("Notify")
            .build()?,
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::MethodReturn)
            .build()?,
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.Notifications")
            .member("NotificationClosed")
            .build()?,
    ])
}

/// The arguments of a `Notify` call as the bus carries them.
#[derive(Debug, serde::Deserialize, zbus::zvariant::Type)]
struct NotifyArgs {
    app_name: String,
    replaces_id: u32,
    app_icon: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    hints: HashMap<String, OwnedVariant>,
    expire_timeout: i32,
}

/// What we keep of a `Notify` call.
struct Pending {
    app_name: String,
    replaces_id: u32,
    app_icon: String,
    summary: String,
    body: String,
    transient: bool,
    origin: Option<String>,
    posted_at: Timestamp,
}

impl From<NotifyArgs> for Pending {
    fn from(a: NotifyArgs) -> Self {
        let transient = a
            .hints
            .get("transient")
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false);
        let origin = a
            .hints
            .get(ORIGIN_HINT)
            .and_then(|v| String::try_from(v.clone()).ok());
        let _ = (a.actions, a.expire_timeout);
        Self {
            app_name: a.app_name,
            replaces_id: a.replaces_id,
            app_icon: a.app_icon,
            summary: a.summary,
            body: a.body,
            transient,
            origin,
            posted_at: Timestamp::now(),
        }
    }
}

impl Pending {
    fn to_notification(&self, id: u32) -> Notification {
        Notification {
            id: id.to_string(),
            app_id: self.app_name.clone(),
            app_name: self.app_name.clone(),
            title: (!self.summary.is_empty()).then(|| self.summary.clone()),
            body: (!self.body.is_empty()).then(|| self.body.clone()),
            posted_at: self.posted_at,
            icon: read_icon(&self.app_icon),
        }
    }
}

/// An icon given as a PNG file path or `file://` URI, within the bound.
fn read_icon(app_icon: &str) -> Option<Icon> {
    let path = app_icon.strip_prefix("file://").unwrap_or(app_icon);
    if !path.starts_with('/') || !path.ends_with(".png") {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    (bytes.len() as u64 <= lp_core::capability::NotificationsParams::DEFAULT_MAX_ICON_BYTES)
        .then(|| Icon::png(bytes))
}

/// Where received icons are written for the notification daemon to read.
fn icon_dir() -> Option<std::path::PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let dir = std::path::Path::new(&runtime)
        .join("ligature")
        .join("icons");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Writes `icon` under `dir` by its hash, once, and returns the path.
fn write_icon(dir: &std::path::Path, icon: &Icon) -> Option<String> {
    let path = dir.join(format!("{}.png", icon.hash));
    if !path.exists() {
        std::fs::write(&path, &icon.bytes).ok()?;
    }
    Some(path.display().to_string())
}
