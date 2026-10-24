//! Notifications on macOS: display only, through the user notification
//! centre. Spec chapter 05 section 3c and chapter 08 section 2b; milestone 3
//! (LP-93), LP-94.
//!
//! The centre serves only a process inside an application bundle, so the
//! daemon runs from `Ligature.app`, whose name the permission prompt and the
//! notifications carry. A copy is posted under an identifier made of the
//! origin's key and the origin's id, so a repeated `post` replaces it and a
//! `dismiss` removes it; the user closing or clicking a copy is reported as
//! [`Event::CopyDismissed`] and travels to the origin and the other devices.
//! A daemon may not read other applications' notifications, so nothing is
//! emitted. Started outside a bundle (a build run from `target/`) or refused
//! permission, the backend reports that it does not display, and the
//! capability says so.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lp_core::capability::NotificationsParams;
use lp_core::{Error, PublicKey};
use lp_macos::notifications::{self as centre, Posting, Response};
use tokio::sync::broadcast;

use crate::notifications::{Event, Notification, Notifications};

/// How many user responses are buffered for the synchroniser.
const EVENT_BUFFER: usize = 16;

/// Where the centre stands for this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Not running from a bundle; the centre is never touched.
    NoBundle,
    /// Asked; the user has not answered.
    Asking,
    /// The user allows notifications.
    Allowed,
    /// The user refused, or the centre failed.
    Refused,
}

/// The macOS backend.
pub struct MacNotifications {
    events: broadcast::Sender<Event>,
    state: Arc<Mutex<State>>,
    icon_dir: PathBuf,
    icon_seq: AtomicU64,
}

impl std::fmt::Debug for MacNotifications {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MacNotifications({:?})", self.state())
    }
}

impl MacNotifications {
    /// Starts the backend: inside a bundle it sets up the centre and asks for
    /// permission; outside one it displays nothing and says why once.
    #[must_use]
    pub fn new() -> Arc<Self> {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let state = Arc::new(Mutex::new(State::Asking));
        let this = Arc::new(Self {
            events,
            state: Arc::clone(&state),
            icon_dir: std::env::temp_dir().join("ligature-icons"),
            icon_seq: AtomicU64::new(0),
        });
        let tx = this.events.clone();
        let responder: centre::Responder = Arc::new(move |response| {
            let identifier = match &response {
                Response::Dismissed(i) | Response::Opened(i) => i,
            };
            if let Some((origin, id)) = parse_identifier(identifier) {
                tracing::debug!(%origin, id, ?response, "copy closed by the user");
                let _ = tx.send(Event::CopyDismissed { origin, id });
            }
        });
        let answered = Arc::clone(&state);
        let authorised: centre::Authorised = Arc::new(move |granted, error| {
            let mut s = answered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *s = if granted {
                tracing::info!("notifications allowed: copies from other devices are shown");
                State::Allowed
            } else {
                tracing::warn!(
                    error = error.as_deref().unwrap_or("refused"),
                    "notifications not allowed: turn on Ligature under System Settings, \
                     Notifications, to see other devices' notifications"
                );
                State::Refused
            };
        });
        if let Err(e) = centre::start(responder, authorised) {
            tracing::warn!(
                %e,
                "notifications from other devices are not displayed: run the daemon from \
                 Ligature.app, as the launchd agent does"
            );
            *this.lock_state() = State::NoBundle;
        }
        this
    }

    fn state(&self) -> State {
        *self.lock_state()
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A fresh copy of the icon for one posting, since the centre moves the
    /// file it is given.
    fn icon_file(&self, notification: &Notification) -> Option<PathBuf> {
        let icon = notification.icon.as_ref()?;
        std::fs::create_dir_all(&self.icon_dir).ok()?;
        let n = self.icon_seq.fetch_add(1, Ordering::Relaxed);
        let path = self.icon_dir.join(format!("{n}-{}.png", icon.hash));
        match std::fs::write(&path, &icon.bytes) {
            Ok(()) => Some(path),
            Err(e) => {
                tracing::debug!(%e, "icon not written; posted without it");
                None
            }
        }
    }
}

/// The centre's identifier of the copy of `origin`'s notification `id`. A key
/// never holds a `/`, so the first one separates the two.
fn identifier(origin: &PublicKey, id: &str) -> String {
    format!("{origin}/{id}")
}

/// The origin and id an [`identifier`] was made from; `None` for anything
/// else.
fn parse_identifier(identifier: &str) -> Option<(PublicKey, String)> {
    let (origin, id) = identifier.split_once('/')?;
    Some((origin.parse().ok()?, id.to_owned()))
}

/// The posting for `notification` from `origin`, named `origin_name`: the app
/// and the device as the title, the notification's title as the subtitle when
/// it has a body, and the body, or the title when there is none, as the text.
/// Copies of one app on one device are grouped.
fn posting(origin: &PublicKey, origin_name: &str, notification: &Notification) -> Posting {
    let text = notification
        .body
        .clone()
        .or_else(|| notification.title.clone());
    let subtitle = notification.body.as_ref().and(notification.title.clone());
    Posting {
        identifier: identifier(origin, &notification.id),
        thread: format!("{origin}/{}", notification.app_id),
        title: format!("{} · {}", notification.app_name, origin_name),
        subtitle,
        body: text,
        image: None,
    }
}

impl Notifications for MacNotifications {
    fn emits(&self) -> bool {
        false
    }

    fn displays(&self) -> bool {
        self.state() == State::Allowed
    }

    fn max_icon_bytes(&self) -> u64 {
        if self.displays() {
            NotificationsParams::DEFAULT_MAX_ICON_BYTES
        } else {
            0
        }
    }

    fn show(
        &self,
        origin: &PublicKey,
        origin_name: &str,
        notification: &Notification,
    ) -> Result<(), Error> {
        if !self.displays() {
            return Err(Error::Io("notifications are not allowed here".to_owned()));
        }
        let mut posting = posting(origin, origin_name, notification);
        posting.image = self.icon_file(notification);
        let image = posting.image.clone();
        centre::post(
            &posting,
            Arc::new(move |e| {
                tracing::warn!(%e, "the notification centre refused a copy");
                if let Some(path) = &image {
                    let _ = std::fs::remove_file(path);
                }
            }),
        );
        Ok(())
    }

    fn dismiss_copy(&self, origin: &PublicKey, id: &str) -> Result<(), Error> {
        if self.state() != State::NoBundle {
            centre::remove(&[&identifier(origin, id)]);
        }
        Ok(())
    }

    fn dismiss_own(&self, _id: &str) -> Result<(), Error> {
        // Nothing is emitted, so nothing is ever ours to cancel.
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lp_core::wire::Timestamp;

    fn key() -> PublicKey {
        "z6MkmCgtFQcDLypvVgsGKV3C5k1EfrpXWTByDcCdW5UyQUmj"
            .parse()
            .unwrap()
    }

    fn notification() -> Notification {
        Notification {
            id: "42/a".to_owned(),
            app_id: "org.chat".to_owned(),
            app_name: "Chat".to_owned(),
            title: Some("Bob".to_owned()),
            body: Some("Lunch?".to_owned()),
            posted_at: Timestamp::now(),
            icon: None,
        }
    }

    #[test]
    fn identifiers_round_trip_even_when_the_id_has_a_slash() {
        let id = identifier(&key(), "42/a");
        assert_eq!(parse_identifier(&id), Some((key(), "42/a".to_owned())));
        assert_eq!(parse_identifier("not-a-key/1"), None);
        assert_eq!(parse_identifier("no-separator"), None);
    }

    #[test]
    fn the_posting_names_the_app_and_device_and_groups_by_app() {
        let p = posting(&key(), "Pixel 8 Pro", &notification());
        assert_eq!(p.title, "Chat · Pixel 8 Pro");
        assert_eq!(p.subtitle.as_deref(), Some("Bob"));
        assert_eq!(p.body.as_deref(), Some("Lunch?"));
        assert_eq!(p.identifier, identifier(&key(), "42/a"));
        assert_eq!(p.thread, format!("{}/org.chat", key()));
    }

    #[test]
    fn a_title_alone_becomes_the_text() {
        let n = Notification {
            body: None,
            ..notification()
        };
        let p = posting(&key(), "tmarchy", &n);
        assert_eq!(p.subtitle, None);
        assert_eq!(p.body.as_deref(), Some("Bob"));
    }

    #[test]
    fn outside_a_bundle_nothing_is_displayed_or_asked_for() {
        // A test binary is not in a bundle, as a build run from target/ is not.
        let n = MacNotifications::new();
        assert!(!n.displays());
        assert!(!n.emits());
        assert_eq!(n.max_icon_bytes(), 0);
        assert!(n.show(&key(), "x", &notification()).is_err());
        assert!(n.dismiss_copy(&key(), "1").is_ok());
    }
}
