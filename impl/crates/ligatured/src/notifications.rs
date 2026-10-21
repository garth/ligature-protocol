//! The local notifications behind one trait. Spec chapter 05 section 3c;
//! milestone 3 (LP-91).
//!
//! A platform backend reports which directions it offers, shows and clears
//! copies of other devices' notifications, cancels this device's own when a
//! dismissal arrives for one, and publishes what happens locally: an app
//! posted or removed a notification (if the platform lets the daemon see
//! them), or the user dismissed a copy. An in-memory fake serves tests and
//! the loopback run.

use std::sync::{Arc, Mutex};

use lp_core::wire::Timestamp;
use lp_core::{Error, PublicKey};
use lp_crypto::hash::sha256;
use tokio::sync::broadcast;

/// An app's icon as it travels: PNG bytes and their hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    /// SHA-256 of `bytes`, base64url without padding.
    pub hash: String,
    /// `image/png`.
    pub mime: String,
    /// The image.
    pub bytes: Vec<u8>,
}

impl Icon {
    /// A PNG icon, hashed.
    #[must_use]
    pub fn png(bytes: Vec<u8>) -> Self {
        let hash = lp_core::wire::Bytes(sha256(&bytes).to_vec()).to_base64url();
        Self {
            hash,
            mime: "image/png".to_owned(),
            bytes,
        }
    }
}

/// A notification as the daemon sees it, on either end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// Chosen by the emitting device, stable for the notification's life.
    pub id: String,
    /// The platform's identifier for the app.
    pub app_id: String,
    /// The app's display name.
    pub app_name: String,
    /// The title, if any.
    pub title: Option<String>,
    /// The body, if any.
    pub body: Option<String>,
    /// When the app posted it.
    pub posted_at: Timestamp,
    /// The app's icon, if there is one.
    pub icon: Option<Icon>,
}

/// Something that happened on the local device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// An app posted or updated a notification this device may forward.
    Posted(Notification),
    /// A notification this device emitted is gone (the user dismissed it or
    /// the app withdrew it); its copies elsewhere should go too.
    Removed {
        /// Its id.
        id: String,
    },
    /// The user dismissed a copy displayed on behalf of `origin`.
    CopyDismissed {
        /// The emitting device.
        origin: PublicKey,
        /// Its id in that device's id space.
        id: String,
    },
}

/// The local notification system. Implementations are cheap to call.
pub trait Notifications: Send + Sync {
    /// Whether this device forwards what its apps post, right now.
    fn emits(&self) -> bool;
    /// Whether this device shows what it is sent.
    fn displays(&self) -> bool;
    /// The largest icon accepted, in bytes; 0 wants none.
    fn max_icon_bytes(&self) -> u64;
    /// Shows, or updates, a copy of a notification from `origin`, whose
    /// display name is `origin_name`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the platform refuses.
    fn show(
        &self,
        origin: &PublicKey,
        origin_name: &str,
        notification: &Notification,
    ) -> Result<(), Error>;
    /// Clears the copy of `origin`'s notification `id`, if it is shown.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the platform refuses.
    fn dismiss_copy(&self, origin: &PublicKey, id: &str) -> Result<(), Error>;
    /// Cancels this device's own notification `id`: another device dismissed
    /// its copy.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the platform refuses.
    fn dismiss_own(&self, id: &str) -> Result<(), Error>;
    /// A receiver of every local event.
    fn subscribe(&self) -> broadcast::Receiver<Event>;
}

/// An in-memory notification system for tests and the loopback run.
pub struct FakeNotifications {
    emits: Mutex<bool>,
    displays: bool,
    max_icon_bytes: u64,
    shown: Mutex<Vec<(PublicKey, String, Notification)>>,
    dismissed_own: Mutex<Vec<String>>,
    events: broadcast::Sender<Event>,
}

impl std::fmt::Debug for FakeNotifications {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FakeNotifications")
    }
}

impl FakeNotifications {
    /// A fake that emits and displays as told, with the default icon bound.
    #[must_use]
    pub fn new(emits: bool, displays: bool) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Self {
            emits: Mutex::new(emits),
            displays,
            max_icon_bytes: lp_core::capability::NotificationsParams::DEFAULT_MAX_ICON_BYTES,
            shown: Mutex::new(Vec::new()),
            dismissed_own: Mutex::new(Vec::new()),
            events,
        })
    }

    /// An app on this device posted `notification`.
    pub fn post(&self, notification: Notification) {
        let _ = self.events.send(Event::Posted(notification));
    }

    /// This device's own notification `id` went away.
    pub fn remove(&self, id: &str) {
        let _ = self.events.send(Event::Removed { id: id.to_owned() });
    }

    /// The user dismissed the displayed copy of `origin`'s `id`.
    pub fn user_dismisses_copy(&self, origin: &PublicKey, id: &str) {
        self.lock(&self.shown)
            .retain(|(o, _, n)| !(o == origin && n.id == id));
        let _ = self.events.send(Event::CopyDismissed {
            origin: *origin,
            id: id.to_owned(),
        });
    }

    /// Whether this device forwards its apps' notifications; flipping it is
    /// the host's way of saying access was granted or revoked.
    pub fn set_emits(&self, emits: bool) {
        *self.lock(&self.emits) = emits;
    }

    /// The copies shown right now: origin, origin's name, notification.
    #[must_use]
    pub fn shown(&self) -> Vec<(PublicKey, String, Notification)> {
        self.lock(&self.shown).clone()
    }

    /// The ids of this device's own notifications that were cancelled from
    /// elsewhere.
    #[must_use]
    pub fn dismissed_own(&self) -> Vec<String> {
        self.lock(&self.dismissed_own).clone()
    }

    fn lock<'a, T>(&self, m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Notifications for FakeNotifications {
    fn emits(&self) -> bool {
        *self.lock(&self.emits)
    }

    fn displays(&self) -> bool {
        self.displays
    }

    fn max_icon_bytes(&self) -> u64 {
        self.max_icon_bytes
    }

    fn show(
        &self,
        origin: &PublicKey,
        origin_name: &str,
        notification: &Notification,
    ) -> Result<(), Error> {
        let mut shown = self.lock(&self.shown);
        shown.retain(|(o, _, n)| !(o == origin && n.id == notification.id));
        shown.push((*origin, origin_name.to_owned(), notification.clone()));
        Ok(())
    }

    fn dismiss_copy(&self, origin: &PublicKey, id: &str) -> Result<(), Error> {
        self.lock(&self.shown)
            .retain(|(o, _, n)| !(o == origin && n.id == id));
        Ok(())
    }

    fn dismiss_own(&self, id: &str) -> Result<(), Error> {
        self.lock(&self.dismissed_own).push(id.to_owned());
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}
