//! Carries notifications between a person's devices. Spec chapter 05 section
//! 3c; milestone 3 (LP-91).
//!
//! What an app posts on this device goes as a `post` to every connected
//! binding of the same person whose descriptor says it displays, after the
//! app's icon has gone once in this session. A `post` received from such a
//! binding is shown by the local backend. A dismissal the user makes here,
//! of this device's own notification or of a copy, goes in one hop to the
//! origin and to every displaying peer; a received `dismiss` is acted on and
//! never forwarded, and a copy is never emitted again, so nothing cycles.
//! Nothing here is stored: the icon caches and id maps live for the session.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use lp_core::capability::{
    NOTIFICATIONS_TYPE, NotificationApp, NotificationDismiss, NotificationIcon,
    NotificationMessage, NotificationPost, NotificationsParams, notifications_descriptor,
};
use lp_core::wire::{Bytes, Timestamp};
use lp_core::{Error, PublicKey};
use lp_identity::trust::Recognition;
use lp_transport::manager::{StreamHandler, open_on};
use lp_transport::quic::Connection;
use tokio::sync::mpsc;

use crate::daemon::Daemon;
use crate::notifications::{Event, Icon, Notification, Notifications};

/// How often the backend's directions are re-read.
const ADVERTISE_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// The notification synchroniser for one daemon.
pub struct NotificationSync {
    daemon: Arc<Daemon>,
    backend: Arc<dyn Notifications>,
    senders: Mutex<HashMap<PublicKey, mpsc::Sender<NotificationMessage>>>,
    /// Icon hashes already sent to each peer this session.
    icons_sent: Mutex<HashMap<PublicKey, HashSet<String>>>,
    /// Icons received from each peer this session, by hash.
    icons_received: Mutex<HashMap<(PublicKey, String), Arc<Icon>>>,
    /// What is advertised right now; held across the registry update so two
    /// concurrent advertisers cannot leave a stale descriptor behind.
    offered: tokio::sync::Mutex<Option<NotificationsParams>>,
    counters: Mutex<Counters>,
}

/// What went out and what came in, for tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// `post` messages sent.
    pub posted: u64,
    /// `post` messages shown.
    pub shown: u64,
    /// `dismiss` messages sent.
    pub dismissed_out: u64,
    /// `dismiss` messages acted on.
    pub dismissed_in: u64,
}

impl std::fmt::Debug for NotificationSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NotificationSync")
    }
}

impl NotificationSync {
    /// Starts synchronising notifications for `daemon` through `backend`:
    /// registers the stream handler, watches local events and advertises
    /// the directions the backend offers.
    #[must_use]
    pub fn start(daemon: Arc<Daemon>, backend: Arc<dyn Notifications>) -> Arc<Self> {
        let this = Arc::new(Self {
            daemon: daemon.clone(),
            backend,
            senders: Mutex::new(HashMap::new()),
            icons_sent: Mutex::new(HashMap::new()),
            icons_received: Mutex::new(HashMap::new()),
            offered: tokio::sync::Mutex::new(None),
            counters: Mutex::new(Counters::default()),
        });
        daemon
            .manager()
            .on_stream(NOTIFICATIONS_TYPE, this.handler());
        let local = this.clone();
        tokio::spawn(async move { local.watch_local().await });
        // The backend's directions can change after start (a bus monitor
        // coming up, access granted on a phone): re-check them every second
        // and tell peers only when they differ, as the clipboard does for
        // its availability.
        let advertise = this.clone();
        tokio::spawn(async move {
            loop {
                advertise.advertise().await;
                tokio::time::sleep(ADVERTISE_POLL).await;
            }
        });
        this
    }

    /// The counters, for tests.
    #[must_use]
    pub fn counters(&self) -> Counters {
        *self.lock(&self.counters)
    }

    /// The directions this device advertises right now.
    #[must_use]
    pub fn params(&self) -> NotificationsParams {
        NotificationsParams {
            emits: self.backend.emits(),
            displays: self.backend.displays(),
            max_icon_bytes: self.backend.max_icon_bytes(),
        }
    }

    /// Re-reads the backend's directions and tells peers if they changed:
    /// for a host whose notification access was granted or revoked.
    pub async fn advertise(&self) {
        let mut offered = self.offered.lock().await;
        let params = self.params();
        if *offered == Some(params) {
            return;
        }
        self.daemon.registry().withdraw(NOTIFICATIONS_TYPE).await;
        if params.emits || params.displays {
            self.daemon
                .registry()
                .offer(notifications_descriptor(params))
                .await;
        }
        *offered = Some(params);
        tracing::info!(
            emits = params.emits,
            displays = params.displays,
            "notifications capability advertised"
        );
    }

    /// A trusted peer connected: open our notifications stream to it if it is
    /// a binding of the same person.
    pub fn attach(self: &Arc<Self>, conn: &Connection) {
        if !self.same_person(conn.peer()) {
            return;
        }
        let peer = *conn.peer();
        let (tx, mut rx) = mpsc::channel::<NotificationMessage>(32);
        self.lock_senders().insert(peer, tx);
        let conn = conn.clone();
        tokio::spawn(async move {
            let mut stream = match open_on(&conn, NOTIFICATIONS_TYPE).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(%peer, %e, "cannot open notifications stream");
                    return;
                }
            };
            while let Some(message) = rx.recv().await {
                if stream.send(&message).await.is_err() {
                    break;
                }
            }
            stream.finish();
        });
    }

    /// A peer disconnected: its session's icons and stream go.
    pub fn detach(&self, peer: &PublicKey) {
        self.lock_senders().remove(peer);
        self.lock(&self.icons_sent).remove(peer);
        self.lock(&self.icons_received)
            .retain(|(p, _), _| p != peer);
    }

    /// An app on this device posted `notification`: send it to every
    /// displaying peer, with its icon first where the peer has not seen it.
    /// Also the path for a test notification from `lpctl notify`.
    pub async fn local_posted(&self, notification: &Notification) {
        if !self.backend.emits() && notification.app_id != LPCTL_APP {
            return;
        }
        let targets = self.displaying_peers();
        for (peer, tx, params) in targets {
            let mut icon_hash = None;
            if let Some(icon) = &notification.icon
                && params.max_icon_bytes >= icon.bytes.len() as u64
            {
                let unseen = self
                    .lock(&self.icons_sent)
                    .entry(peer)
                    .or_default()
                    .insert(icon.hash.clone());
                if unseen {
                    let message = NotificationMessage::Icon(NotificationIcon {
                        hash: icon.hash.clone(),
                        mime: icon.mime.clone(),
                        bytes: Bytes(icon.bytes.clone()),
                    });
                    if tx.send(message).await.is_err() {
                        continue;
                    }
                }
                icon_hash = Some(icon.hash.clone());
            }
            let post = NotificationMessage::Post(NotificationPost {
                id: notification.id.clone(),
                app: NotificationApp {
                    id: notification.app_id.clone(),
                    name: notification.app_name.clone(),
                },
                title: notification.title.clone(),
                body: notification.body.clone(),
                posted_at: notification.posted_at,
                icon: icon_hash,
            });
            if tx.send(post).await.is_ok() {
                self.lock(&self.counters).posted += 1;
                tracing::debug!(%peer, id = %notification.id, "notification sent");
            }
        }
    }

    /// The user dismissed `origin`'s notification `id` here: tell the origin
    /// and every displaying peer, in one hop.
    pub async fn local_dismissed(&self, origin: &PublicKey, id: &str) {
        let dismiss = NotificationMessage::Dismiss(NotificationDismiss {
            origin: *origin,
            id: id.to_owned(),
            dismissed_at: Timestamp::now(),
        });
        let mut targets: Vec<(PublicKey, mpsc::Sender<NotificationMessage>)> = self
            .displaying_peers()
            .into_iter()
            .map(|(p, tx, _)| (p, tx))
            .collect();
        if origin != self.daemon.binding().public()
            && !targets.iter().any(|(p, _)| p == origin)
            && self.same_person(origin)
            && let Some(tx) = self.lock_senders().get(origin).cloned()
        {
            targets.push((*origin, tx));
        }
        for (peer, tx) in targets {
            if tx.send(dismiss.clone()).await.is_ok() {
                self.lock(&self.counters).dismissed_out += 1;
                tracing::debug!(%peer, id, "dismissal sent");
            }
        }
    }

    /// Handles one message from `peer`. Returns what was done, for logs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] if the peer is not a binding of this person,
    /// [`Error::Decode`] for an icon over the bound, and [`Error::Io`] if the
    /// backend refuses.
    pub fn apply(
        &self,
        peer: &PublicKey,
        message: NotificationMessage,
    ) -> Result<&'static str, Error> {
        if !self.same_person(peer) {
            return Err(Error::Chain(
                "notification from a device that is not mine".to_owned(),
            ));
        }
        match message {
            NotificationMessage::Icon(icon) => {
                if icon.bytes.0.len() as u64 > self.backend.max_icon_bytes() {
                    return Err(Error::Decode("icon larger than accepted".to_owned()));
                }
                let expected = Icon::png(icon.bytes.0.clone()).hash;
                if expected != icon.hash {
                    return Err(Error::Decode("icon hash does not match".to_owned()));
                }
                self.lock(&self.icons_received).insert(
                    (*peer, icon.hash.clone()),
                    Arc::new(Icon {
                        hash: icon.hash,
                        mime: icon.mime,
                        bytes: icon.bytes.0,
                    }),
                );
                Ok("icon cached")
            }
            NotificationMessage::Post(post) => {
                if !self.backend.displays() {
                    return Ok("not displaying");
                }
                let icon = post.icon.as_ref().and_then(|h| {
                    self.lock(&self.icons_received)
                        .get(&(*peer, h.clone()))
                        .map(|i| (**i).clone())
                });
                let notification = Notification {
                    id: post.id,
                    app_id: post.app.id,
                    app_name: post.app.name,
                    title: post.title,
                    body: post.body,
                    posted_at: post.posted_at,
                    icon,
                };
                let origin_name = self
                    .daemon
                    .device_display_name(peer)
                    .unwrap_or_else(|| peer.fingerprint());
                self.backend.show(peer, &origin_name, &notification)?;
                self.lock(&self.counters).shown += 1;
                crate::grants::record_implicit_for(&self.daemon, peer, NOTIFICATIONS_TYPE);
                Ok("shown")
            }
            NotificationMessage::Dismiss(dismiss) => {
                if dismiss.origin == *self.daemon.binding().public() {
                    self.backend.dismiss_own(&dismiss.id)?;
                } else {
                    self.backend.dismiss_copy(&dismiss.origin, &dismiss.id)?;
                }
                self.lock(&self.counters).dismissed_in += 1;
                Ok("dismissed")
            }
        }
    }

    /// Connected same-person peers whose descriptor says they display, with
    /// their params.
    fn displaying_peers(
        &self,
    ) -> Vec<(
        PublicKey,
        mpsc::Sender<NotificationMessage>,
        NotificationsParams,
    )> {
        let remote: HashMap<PublicKey, NotificationsParams> = self
            .daemon
            .registry()
            .peers()
            .into_iter()
            .filter_map(|caps| {
                caps.capabilities
                    .iter()
                    .find_map(NotificationsParams::from_descriptor)
                    .filter(|p| p.displays)
                    .map(|p| (caps.peer, p))
            })
            .collect();
        self.lock_senders()
            .iter()
            .filter(|(peer, _)| self.same_person(peer))
            .filter_map(|(peer, tx)| remote.get(peer).map(|p| (*peer, tx.clone(), *p)))
            .collect()
    }

    fn same_person(&self, peer: &PublicKey) -> bool {
        matches!(
            self.daemon.recognise(peer),
            Ok(Recognition::MyBinding { .. })
        )
    }

    async fn watch_local(self: Arc<Self>) {
        let mut events = self.backend.subscribe();
        loop {
            let event = match events.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            match event {
                Event::Posted(notification) => self.local_posted(&notification).await,
                Event::Removed { id } => {
                    let me = *self.daemon.binding().public();
                    self.local_dismissed(&me, &id).await;
                }
                Event::CopyDismissed { origin, id } => self.local_dismissed(&origin, &id).await,
            }
        }
    }

    fn handler(self: &Arc<Self>) -> StreamHandler {
        let sync = self.clone();
        Arc::new(move |conn, mut stream| {
            let sync = sync.clone();
            Box::pin(async move {
                let peer = *conn.peer();
                loop {
                    match stream.recv::<NotificationMessage>().await {
                        Ok(Some(message)) => match sync.apply(&peer, message) {
                            Ok(what) => tracing::debug!(%peer, what, "notification message"),
                            Err(e) => {
                                tracing::warn!(%peer, %e, "notification message refused");
                                stream.reset(lp_core::frame::REFUSED_CODE);
                                break;
                            }
                        },
                        Ok(None) => break,
                        Err(e) => {
                            tracing::debug!(%peer, %e, "notifications stream ended");
                            break;
                        }
                    }
                }
            })
        })
    }

    fn lock<'a, T>(&self, m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_senders(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<PublicKey, mpsc::Sender<NotificationMessage>>> {
        self.senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The app id `lpctl notify` posts under; such a post goes out even on a
/// device that does not emit, since the user asked for it.
pub const LPCTL_APP: &str = "org.ligatureprotocol.lpctl";
