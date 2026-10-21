//! The host's notifications behind `ligatured`'s [`Notifications`] trait.
//!
//! Android lets only the app see notifications, through a listener service
//! the user enables, and only the app can post them. So the host tells the
//! node what its apps posted or removed and when the user dismissed a copy,
//! and the node asks the host to show, update and clear copies and to cancel
//! one of the phone's own. Whether the listener is connected is the host's
//! to report; the capability's `emits` follows it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ligatured::notifications::{Event, Icon, Notification, Notifications};
use lp_core::wire::Timestamp;
use lp_core::{Error, PublicKey};
use tokio::sync::broadcast;

use crate::{LocalNotification, NotificationCopy, SharedHost};

/// The notification system the host feeds and drives.
pub struct HostNotifications {
    host: SharedHost,
    emits: AtomicBool,
    events: broadcast::Sender<Event>,
}

impl std::fmt::Debug for HostNotifications {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostNotifications")
    }
}

impl HostNotifications {
    /// A backend that displays through `host` and emits nothing until told
    /// the listener is connected.
    #[must_use]
    pub fn new(host: SharedHost) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Self {
            host,
            emits: AtomicBool::new(false),
            events,
        })
    }

    /// The listener connected or disconnected.
    pub fn set_emits(&self, emits: bool) {
        self.emits.store(emits, Ordering::SeqCst);
    }

    /// An app on the phone posted or updated `notification`.
    pub fn posted(&self, notification: LocalNotification) {
        let _ = self.events.send(Event::Posted(Notification {
            id: notification.id,
            app_id: notification.app_id,
            app_name: notification.app_name,
            title: notification.title,
            body: notification.body,
            posted_at: Timestamp::from_unix(notification.posted_at_unix)
                .unwrap_or_else(|_| Timestamp::now()),
            icon: notification.icon_png.map(Icon::png),
        }));
    }

    /// One of the phone's own notifications went away.
    pub fn removed(&self, id: String) {
        let _ = self.events.send(Event::Removed { id });
    }

    /// The user dismissed the copy of `origin`'s `id`.
    pub fn copy_dismissed(&self, origin: PublicKey, id: String) {
        let _ = self.events.send(Event::CopyDismissed { origin, id });
    }
}

impl Notifications for HostNotifications {
    fn emits(&self) -> bool {
        self.emits.load(Ordering::SeqCst)
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
        let copy = NotificationCopy {
            origin: origin.to_string(),
            origin_name: origin_name.to_owned(),
            id: notification.id.clone(),
            app_id: notification.app_id.clone(),
            app_name: notification.app_name.clone(),
            title: notification.title.clone(),
            body: notification.body.clone(),
            posted_at_unix: notification.posted_at.unix(),
            icon_png: notification.icon.as_ref().map(|i| i.bytes.clone()),
        };
        if self.host.show_notification(copy) {
            Ok(())
        } else {
            Err(Error::Io(
                "the host refused to show the notification".to_owned(),
            ))
        }
    }

    fn dismiss_copy(&self, origin: &PublicKey, id: &str) -> Result<(), Error> {
        if self
            .host
            .dismiss_notification_copy(origin.to_string(), id.to_owned())
        {
            Ok(())
        } else {
            Err(Error::Io("the host refused to clear the copy".to_owned()))
        }
    }

    fn dismiss_own(&self, id: &str) -> Result<(), Error> {
        if self.host.dismiss_own_notification(id.to_owned()) {
            Ok(())
        } else {
            Err(Error::Io(
                "the host could not cancel the notification".to_owned(),
            ))
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}
