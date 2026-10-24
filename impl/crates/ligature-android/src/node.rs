//! The node: a daemon on its own runtime, driven by the host's calls.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ligatured::ceremony::{self, Prompt};
use ligatured::clipboard::Clipboard as _;
use ligatured::daemon::Daemon;
use ligatured::run::{
    RECONNECT_EVERY, default_bind, spawn_connection_events, spawn_discovery, spawn_reconnect,
};
use lp_core::PublicKey;
use lp_core::paths::Locations;
use lp_transport::manager::Event as ConnectionEvent;
use tokio::sync::{mpsc, oneshot};

use ligatured::files::{Destination, FileSource};
use ligatured::files_sync::{FileState, FilesConfig, TransferEnd, TransferProgress};
use lp_core::files::{FILES_TYPE, FileDescriptor, FilesParams};
use lp_core::wire::Timestamp;

use crate::clipboard::HostClipboard;
use crate::pairing::Pairing;
use crate::{Event, LpError, SharedHost, logging};

/// How to start a node.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Config {
    /// Directory for the store and the key file: the app's files directory.
    pub data_dir: String,
    /// This device's display name for a device that has no record yet.
    pub device_name: String,
    /// Log at `debug` instead of `info`.
    pub debug_logging: bool,
    /// Where fetched clipboard content is cached before a paste completes:
    /// the app's cache directory. The system's temporary directory when
    /// absent, which an app cannot write.
    #[uniffi(default = None)]
    pub cache_dir: Option<String>,
}

/// What the node is right now.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Status {
    /// Whether the node has an identity.
    pub initialised: bool,
    /// The daemon's version.
    pub version: String,
    /// The person's master key, once initialised.
    pub person: Option<String>,
    /// The person's display name, once initialised.
    pub name: Option<String>,
    /// This device's binding key.
    pub binding: String,
    /// This device's principal key.
    pub principal: String,
    /// A short fingerprint of the binding key, for display only.
    pub fingerprint: String,
    /// This device's display name.
    pub device_name: String,
    /// The UDP port the QUIC endpoint is bound to.
    pub port: u16,
    /// Peers with a live, recognised connection.
    pub peers: Vec<String>,
    /// Peers whose live manifest offers the clipboard.
    pub clipboard_peers: Vec<String>,
    /// Peers whose live manifest says they receive files.
    pub file_peers: Vec<String>,
}

/// A file the host hands the node to send: content it opened, as a path the
/// node can read (`/proc/self/fd/N` for a descriptor the host holds open
/// until the transfer ends, or a copy in the cache).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OutgoingFile {
    /// The path to read.
    pub path: String,
    /// The name the receiver should use.
    pub name: String,
    /// The exact size.
    pub size: u64,
    /// The content type, or empty when unknown.
    pub mime: String,
    /// When the content was last modified, in Unix seconds, when the
    /// provider says.
    pub modified: Option<i64>,
}

/// A transfer as the host shows it: one notification's worth.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TransferInfo {
    /// The transfer id.
    pub transfer: String,
    /// The other device's binding key.
    pub peer: String,
    /// The other device's display name.
    pub peer_name: String,
    /// Whether this device sends the bytes.
    pub outgoing: bool,
    /// Bytes to move over every file.
    pub total_bytes: u64,
    /// Bytes moved so far.
    pub done_bytes: u64,
    /// How many files the transfer has.
    pub files_total: u32,
    /// How many are received, failed or cancelled.
    pub files_done: u32,
    /// The first file's name, for a title.
    pub first_name: String,
    /// The names of the files moving right now.
    pub active_names: Vec<String>,
    /// How it ended, once it has: `complete`, `declined: <reason>`,
    /// `cancelled` or `failed: <message>`.
    pub outcome: Option<String>,
    /// Whether every file was received.
    pub all_received: bool,
    /// Where received files landed, for opening them.
    pub locations: Vec<String>,
}

impl TransferInfo {
    fn from_progress(p: &TransferProgress, peer_name: String) -> Self {
        let done = |f: &ligatured::files_sync::FileProgress| {
            !matches!(f.state, FileState::Pending | FileState::Active)
        };
        Self {
            transfer: p.transfer.clone(),
            peer: p.peer.to_string(),
            peer_name,
            outgoing: p.outgoing,
            total_bytes: p.total_bytes(),
            done_bytes: p.done_bytes(),
            files_total: u32::try_from(p.files.len()).unwrap_or(u32::MAX),
            files_done: u32::try_from(p.files.iter().filter(|f| done(f)).count())
                .unwrap_or(u32::MAX),
            first_name: p.files.first().map(|f| f.name.clone()).unwrap_or_default(),
            active_names: p
                .files
                .iter()
                .filter(|f| f.state == FileState::Active)
                .map(|f| f.name.clone())
                .collect(),
            outcome: p.finished.as_ref().map(|end| match end {
                TransferEnd::Complete => "complete".to_owned(),
                TransferEnd::Declined { reason } => format!("declined: {reason}"),
                TransferEnd::Cancelled => "cancelled".to_owned(),
                TransferEnd::Failed { message } => format!("failed: {message}"),
            }),
            all_received: p.all_received(),
            locations: p
                .files
                .iter()
                .filter_map(|f| match &f.state {
                    FileState::Received { location } => location.clone(),
                    _ => None,
                })
                .collect(),
        }
    }
}

/// One device of the person, as the trust store knows it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Device {
    /// The device key.
    pub key: String,
    /// A short fingerprint of the key, for display only.
    pub fingerprint: String,
    /// `binding` or `principal`.
    pub role: String,
    /// For a binding: whether it holds the self-signing key.
    pub introducer: bool,
    /// From its device record, if known.
    pub display_name: Option<String>,
    /// From its device record, if known.
    pub platform: Option<String>,
    /// Whether a revocation names it.
    pub revoked: bool,
    /// Whether it is connected right now.
    pub connected: bool,
    /// Whether it is this device.
    pub this_device: bool,
    /// For a binding: the principal key of the device it lives on, if its
    /// record names one, so the screen can show one row per device.
    pub principal: Option<String>,
}

/// A running daemon. One per process at a time; [`Node::stop`] ends it.
#[derive(uniffi::Object)]
pub struct Node {
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    daemon: Arc<Daemon>,
    clipboard: Arc<HostClipboard>,
    notifications: Arc<crate::HostNotifications>,
    files: Arc<ligatured::files_sync::FilesSync>,
    host: SharedHost,
    /// The phone's screen, shared through the app (milestone 8).
    screen: Arc<crate::HostScreen>,
    /// The share this phone watches: the sharer and its control.
    watching: Arc<Mutex<WatchSlot>>,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Node({})", self.daemon.binding().public())
    }
}

/// How long a content provider read waits for a promised part or file to
/// arrive before answering with nothing (LP-127).
const CLIPBOARD_FETCH_TIMEOUT: Duration = Duration::from_secs(300);

#[uniffi::export]
impl Node {
    /// Opens or creates the state under `config.data_dir`, binds the QUIC
    /// endpoint on an assigned port, attaches the host's clipboard and starts
    /// discovery. Log lines go to `host` from here on.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::Internal`] if the store, the key file or the
    /// endpoint cannot be opened.
    #[uniffi::constructor]
    pub fn start(config: Config, host: Arc<dyn crate::Host>) -> Result<Arc<Self>, LpError> {
        logging::install(host.clone(), config.debug_logging);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("ligature")
            .build()
            .map_err(|e| LpError::Internal(format!("runtime: {e}")))?;
        let locations = Locations::resolve(Some(&PathBuf::from(&config.data_dir)), None);
        let device_name = config.device_name.trim();
        let device_name = (!device_name.is_empty()).then_some(device_name);
        let (daemon, events) = {
            let _guard = runtime.enter();
            Daemon::open(locations, default_bind(), device_name)?
        };
        tracing::info!(
            version = env!("CARGO_PKG_VERSION"),
            binding = %daemon.binding().public(),
            initialised = daemon.is_initialised(),
            port = daemon.status().port,
            "node running"
        );
        let clipboard = HostClipboard::new(host.clone());
        let notifications = crate::HostNotifications::new(host.clone());
        let files = {
            let _guard = runtime.enter();
            // One file at a time onto the phone's storage; up to four at a
            // time off it.
            daemon.attach_files(
                crate::HostFiles::new(host.clone()),
                FilesConfig {
                    max_streams: 1,
                    max_chunk_bytes: FilesParams::DEFAULT_MAX_CHUNK_BYTES,
                    send_streams: 4,
                    // The app's transfer notification says the file landed.
                    announce_received: false,
                },
            )
        };
        let node = Arc::new(Self {
            runtime: Mutex::new(Some(runtime)),
            daemon,
            clipboard: clipboard.clone(),
            notifications: notifications.clone(),
            files: files.clone(),
            screen: crate::HostScreen::new(host.clone()),
            watching: Arc::new(Mutex::new(None)),
            host,
        });
        let clipboard_config = ligatured::clipboard_sync::ClipboardConfig {
            cache_dir: config.cache_dir.as_deref().map_or_else(
                || ligatured::clipboard::cache_dir(node.daemon.locations()),
                |d| PathBuf::from(d).join("clipboard"),
            ),
            ..ligatured::clipboard_sync::ClipboardConfig::default()
        };
        node.with_runtime(|rt| {
            let _guard = rt.enter();
            node.daemon
                .attach_clipboard_with(clipboard, clipboard_config);
            node.daemon.attach_notifications(notifications);
            // The phone shares its screen through the app's media projection
            // and watches a desktop's in the app (milestone 8).
            node.daemon.attach_screen(
                Some(node.screen.clone()),
                Some(lp_core::screen::ScreenParams {
                    views: true,
                    ..lp_core::screen::ScreenParams::default()
                }),
            );
            // The phone drives a desktop as a touchpad (milestone 7): it
            // provides input and accepts none, since Android lets an app
            // inject nothing.
            let input = node
                .daemon
                .attach_input(Some(ligatured::input::Touchpad::new()), None);
            let host = node.host.clone();
            let mut changes = input.subscribe();
            rt.spawn(async move {
                loop {
                    match changes.recv().await {
                        Ok(ligatured::input::Change::StoppedDriving(peer)) => {
                            host.on_event(Event::InputEnded {
                                peer: peer.to_string(),
                            });
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            });
            // Every transfer, either direction, is shown by the host as it
            // moves: at most a few updates a second, and always its end.
            let host = node.host.clone();
            let daemon = node.daemon.clone();
            let mut started = files.subscribe();
            rt.spawn(async move {
                loop {
                    let transfer = match started.recv().await {
                        Ok(t) => t,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    };
                    let host = host.clone();
                    let daemon = daemon.clone();
                    tokio::spawn(async move {
                        let peer = transfer.progress().peer;
                        let peer_name = daemon
                            .device_display_name(&peer)
                            .unwrap_or_else(|| peer.fingerprint());
                        let mut rx = transfer.watch();
                        let mut last = Instant::now() - Duration::from_secs(1);
                        loop {
                            let progress = rx.borrow_and_update().clone();
                            let finished = progress.finished.is_some();
                            if finished || last.elapsed() >= Duration::from_millis(250) {
                                last = Instant::now();
                                host.on_event(Event::TransferChanged {
                                    info: TransferInfo::from_progress(&progress, peer_name.clone()),
                                });
                            }
                            if finished || rx.changed().await.is_err() {
                                return;
                            }
                        }
                    });
                }
            });
            // Tee the connection events: the host learns of every change,
            // the daemon's own loop handles them.
            let (tx, rx) = mpsc::channel::<ConnectionEvent>(32);
            let host = node.host.clone();
            rt.spawn(async move {
                let mut events = events;
                while let Some(event) = events.recv().await {
                    let forward = tx.send(event).await;
                    host.on_event(Event::StatusChanged);
                    if forward.is_err() {
                        break;
                    }
                }
            });
            spawn_connection_events(node.daemon.clone(), rx);
            spawn_discovery(node.daemon.clone());
            spawn_reconnect(node.daemon.clone(), RECONNECT_EVERY);
            // Manifests arrive after the connection: tell the host again
            // once they can have, so the clipboard peers show up.
            let host = node.host.clone();
            let daemon = node.daemon.clone();
            rt.spawn(async move {
                let mut last = Vec::new();
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let mut now = daemon
                        .registry()
                        .peers_offering(lp_core::capability::CLIPBOARD_TYPE);
                    now.extend(daemon.registry().peers_offering(FILES_TYPE));
                    now.extend(daemon.registry().peers_offering(lp_core::input::INPUT_TYPE));
                    now.extend(
                        daemon
                            .registry()
                            .peers_offering(lp_core::screen::SCREEN_TYPE),
                    );
                    if now != last {
                        last = now;
                        host.on_event(Event::StatusChanged);
                    }
                }
            });
        })?;
        Ok(node)
    }

    /// The devices this phone can drive as a touchpad: this person's
    /// connected devices whose input descriptor says they accept.
    #[must_use]
    pub fn input_targets(&self) -> Vec<crate::InputTarget> {
        use lp_core::input::InputParams;
        let mut targets: Vec<crate::InputTarget> = self
            .daemon
            .registry()
            .peers()
            .into_iter()
            .filter(|caps| {
                matches!(
                    self.daemon.recognise(&caps.peer),
                    Ok(lp_identity::trust::Recognition::MyBinding { .. })
                )
            })
            .filter_map(|caps| {
                caps.capabilities
                    .iter()
                    .find_map(InputParams::from_descriptor)
                    .filter(|p| p.accepts)
                    .map(|p| crate::InputTarget {
                        key: caps.peer.to_string(),
                        name: self
                            .daemon
                            .device_display_name(&caps.peer)
                            .unwrap_or_else(|| caps.peer.fingerprint()),
                        text: p.text,
                    })
            })
            .collect();
        targets.sort_by(|a, b| a.name.cmp(&b.name).then(a.key.cmp(&b.key)));
        targets
    }

    /// Starts driving `peer` as a touchpad: an absolute entry at the middle
    /// of its screen. Returns that screen's size.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::BadRequest`] for a malformed key,
    /// [`LpError::Mismatch`] for a device that is not this person's or does
    /// not accept, and [`LpError::Internal`] when it is not connected or a
    /// drive already runs.
    pub fn input_start(&self, peer: String) -> Result<crate::InputScreen, LpError> {
        let peer = PublicKey::parse(peer.trim())?;
        let sync = self.input_sync()?;
        let ready = self.with_runtime(|rt| {
            rt.block_on(sync.drive(peer, lp_core::input::Enter::absolute(0.5, 0.5)))
        })??;
        Ok(crate::InputScreen {
            width: ready.width,
            height: ready.height,
        })
    }

    /// Starts driving `peer` with the pointer at `x`, `y`, fractions of its
    /// screen: the watch screen's first touch. Returns the screen's size.
    ///
    /// # Errors
    ///
    /// As [`Node::input_start`].
    pub fn input_start_at(
        &self,
        peer: String,
        x: f64,
        y: f64,
    ) -> Result<crate::InputScreen, LpError> {
        let peer = PublicKey::parse(peer.trim())?;
        let sync = self.input_sync()?;
        let ready = self.with_runtime(|rt| {
            rt.block_on(sync.drive(peer, lp_core::input::Enter::absolute(x, y)))
        })??;
        Ok(crate::InputScreen {
            width: ready.width,
            height: ready.height,
        })
    }

    /// The devices whose screen this phone can watch: this person's
    /// connected devices whose descriptor says they share.
    #[must_use]
    pub fn screen_targets(&self) -> Vec<crate::ScreenTarget> {
        use lp_core::screen::ScreenParams;
        let mut targets: Vec<crate::ScreenTarget> = self
            .daemon
            .registry()
            .peers()
            .into_iter()
            .filter(|caps| {
                matches!(
                    self.daemon.recognise(&caps.peer),
                    Ok(lp_identity::trust::Recognition::MyBinding { .. })
                )
            })
            .filter(|caps| {
                caps.capabilities
                    .iter()
                    .find_map(ScreenParams::from_descriptor)
                    .is_some_and(|p| p.shares)
            })
            .map(|caps| crate::ScreenTarget {
                key: caps.peer.to_string(),
                name: self
                    .daemon
                    .device_display_name(&caps.peer)
                    .unwrap_or_else(|| caps.peer.fingerprint()),
            })
            .collect();
        targets.sort_by(|a, b| a.name.cmp(&b.name).then(a.key.cmp(&b.key)));
        targets
    }

    /// Watches `peer`'s primary display, no larger than `max_width` by
    /// `max_height` and no faster than `frame_rate`. Frames go to
    /// [`crate::Host::screen_frame`] and the end to
    /// [`Event::ScreenEnded`]. Blocks until the sharer answers, which may
    /// wait on its consent.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::BadRequest`] for a malformed key,
    /// [`LpError::Mismatch`] for a device that does not share or refused,
    /// and [`LpError::Internal`] when a watch already runs or the stream fails.
    pub fn screen_watch(
        &self,
        peer: String,
        max_width: u32,
        max_height: u32,
        frame_rate: u32,
    ) -> Result<crate::ScreenWatch, LpError> {
        let peer = PublicKey::parse(peer.trim())?;
        if lock_watch(&self.watching).is_some() {
            return Err(LpError::Internal("already watching a screen".to_owned()));
        }
        let sync = self
            .daemon
            .screen_sync()
            .ok_or_else(|| LpError::Internal("screen is not attached".to_owned()))?;
        let start = lp_core::screen::Start {
            display: None,
            codec: lp_core::screen::Codec::H264,
            width: max_width,
            height: max_height,
            frame_rate,
        };
        let watching = self.with_runtime(|rt| rt.block_on(sync.watch(&peer, start)))??;
        let ligatured::screen::Watching {
            started,
            mut frames,
            control,
        } = watching;
        *lock_watch(&self.watching) = Some((peer, control));
        let host = self.host.clone();
        let peer_s = peer.to_string();
        let watching = self.watching_slot();
        self.with_runtime(|rt| {
            rt.spawn(async move {
                while let Some(viewed) = frames.recv().await {
                    match viewed {
                        ligatured::screen::Viewed::Frame { key, bytes, .. } => {
                            host.screen_frame(peer_s.clone(), key, bytes);
                        }
                        ligatured::screen::Viewed::Stopped(reason) => {
                            if let Some(slot) = watching.upgrade() {
                                lock_watch(&slot).take();
                            }
                            host.on_event(Event::ScreenEnded {
                                peer: peer_s.clone(),
                                reason: reason.to_string(),
                            });
                            return;
                        }
                    }
                }
            });
        })?;
        Ok(crate::ScreenWatch {
            display: started.display,
            width: started.width,
            height: started.height,
            frame_rate: started.frame_rate,
        })
    }

    /// Asks the sharer of the watched screen for a keyframe: the decoder
    /// lost its place.
    pub fn screen_watch_keyframe(&self) {
        if let Some((_, control)) = lock_watch(&self.watching).as_ref() {
            let _ = control.send(ligatured::screen::ShareControl::Keyframe);
        }
    }

    /// Stops watching: the user left the watch screen.
    pub fn screen_watch_stop(&self) {
        if let Some((_, control)) = lock_watch(&self.watching).take() {
            let _ = control.send(ligatured::screen::ShareControl::Stop);
        }
    }

    /// Whether the app can share the screen (it has an H.264 encoder) and
    /// the screen's size in pixels.
    pub fn set_screen_share(&self, available: bool, width: u32, height: u32) {
        self.screen.set_available(available, width, height);
    }

    /// The user answered the media projection dialog.
    pub fn screen_share_started(&self, ok: bool) {
        self.screen.started(ok);
    }

    /// One encoded access unit of this phone's screen.
    pub fn screen_share_frame(&self, key: bool, bytes: Vec<u8>) {
        self.screen.frame(key, bytes);
    }

    /// The share of this phone's screen ended on the phone: `consent`,
    /// `sharer` or `error`.
    pub fn screen_share_ended(&self, reason: String) {
        self.screen.ended(&reason);
    }

    /// Sends what the touchpad screen did to the device being driven, as one
    /// batch: motion in the driven screen's pixels, buttons, scroll steps,
    /// keys and text.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::Internal`] when nothing is being driven.
    pub fn input_send(&self, events: Vec<crate::TouchEvent>) -> Result<(), LpError> {
        use lp_core::input::InputEvent;
        let events: Vec<InputEvent> = events
            .into_iter()
            .map(|e| match e {
                crate::TouchEvent::Motion { dx, dy } => InputEvent::Motion { dx, dy },
                crate::TouchEvent::Position { x, y } => InputEvent::Position { x, y },
                crate::TouchEvent::Button { code, pressed } => InputEvent::Button { code, pressed },
                crate::TouchEvent::Scroll { dx, dy } => InputEvent::Scroll {
                    dx,
                    dy,
                    discrete: true,
                },
                crate::TouchEvent::Key { code, pressed } => InputEvent::Key { code, pressed },
                crate::TouchEvent::Text { text } => InputEvent::Text { text },
            })
            .collect();
        if events.is_empty() {
            return Ok(());
        }
        let sync = self.input_sync()?;
        self.with_runtime(|rt| rt.block_on(sync.send(events, None)))??;
        Ok(())
    }

    /// Ends the drive, lifting everything on the driven device: the user
    /// left the touchpad screen.
    pub fn input_stop(&self) {
        if let Ok(sync) = self.input_sync() {
            let _ = self.with_runtime(|rt| rt.block_on(sync.release_local(None)));
        }
    }

    /// The device being driven, if any.
    #[must_use]
    pub fn input_driving(&self) -> Option<String> {
        self.daemon
            .input_sync()
            .and_then(|s| s.status().driving)
            .map(|d| d.peer.to_string())
    }

    /// The current status.
    #[must_use]
    pub fn status(&self) -> Status {
        let s = self.daemon.status();
        let mut clipboard_peers: Vec<String> = self
            .daemon
            .registry()
            .peers_offering(lp_core::capability::CLIPBOARD_TYPE)
            .iter()
            .map(ToString::to_string)
            .collect();
        clipboard_peers.sort();
        let mut file_peers: Vec<String> = self
            .daemon
            .registry()
            .peers()
            .into_iter()
            .filter(|c| {
                c.capabilities
                    .iter()
                    .find_map(FilesParams::from_descriptor)
                    .is_some_and(|p| p.receive)
            })
            .map(|c| c.peer.to_string())
            .collect();
        file_peers.sort();
        Status {
            initialised: s.initialised,
            version: s.version,
            person: s.person.map(|k| k.to_string()),
            name: s.name,
            binding: s.binding.to_string(),
            principal: s.principal.to_string(),
            fingerprint: s.binding.fingerprint(),
            device_name: s.device_name,
            port: s.port,
            peers: s.peers.iter().map(ToString::to_string).collect(),
            clipboard_peers,
            file_peers,
        }
    }

    /// Sends `files` to `peer`, one of this person's devices that receives
    /// files. Returns the transfer id; progress arrives as
    /// [`Event::TransferChanged`]. The host keeps what is behind each path
    /// open until the transfer's last event.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::BadRequest`] for a malformed key, an empty list or
    /// a peer that does not receive files, [`LpError::Mismatch`] for a peer
    /// that is not this person's, and [`LpError::Internal`] if it is not
    /// connected.
    pub fn send_files(&self, peer: String, files: Vec<OutgoingFile>) -> Result<String, LpError> {
        let peer = PublicKey::parse(peer.trim())?;
        let mut list = Vec::with_capacity(files.len());
        for f in files {
            let id = ligatured::files_sync::FilesSync::new_id()?;
            let mut descriptor =
                FileDescriptor::new(&id, &ligatured::files::safe_name(&f.name), f.size);
            descriptor.mime = if f.mime.trim().is_empty() {
                ligatured::files::mime_for(&f.name).to_owned()
            } else {
                f.mime
            };
            descriptor.modified = f.modified.and_then(|t| Timestamp::from_unix(t).ok());
            list.push((descriptor, FileSource::Path(PathBuf::from(f.path))));
        }
        let transfer = self.with_runtime(|rt| {
            let _guard = rt.enter();
            self.files.push(&peer, list)
        })??;
        Ok(transfer.id())
    }

    /// Cancels a transfer in flight, either direction. Returns whether there
    /// was one.
    pub fn cancel_transfer(&self, transfer: String) -> bool {
        self.files.cancel(&transfer)
    }

    /// The transfers in flight, for a host that restarts its notifications.
    #[must_use]
    pub fn transfers(&self) -> Vec<TransferInfo> {
        self.files
            .transfers()
            .iter()
            .map(|p| {
                let name = self
                    .daemon
                    .device_display_name(&p.peer)
                    .unwrap_or_else(|| p.peer.fingerprint());
                TransferInfo::from_progress(p, name)
            })
            .collect()
    }

    /// Creates the person on this first device (`lpctl init`). Returns the
    /// master key.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::AlreadyInitialised`], [`LpError::BadRequest`] for
    /// an empty name, or a store error.
    pub fn init(&self, person_name: String) -> Result<String, LpError> {
        let name = person_name.trim();
        if name.is_empty() {
            return Err(LpError::BadRequest("a name is required".to_owned()));
        }
        let person = self.with_runtime(|rt| {
            let _guard = rt.enter();
            self.daemon.init(name, None)
        })??;
        self.host.on_event(Event::StatusChanged);
        Ok(person.to_string())
    }

    /// The person's devices from the trust store.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::NotInitialised`] or a store error.
    pub fn devices(&self) -> Result<Vec<Device>, LpError> {
        let listed = ligatured::control::devices(&self.daemon)?;
        Ok(listed
            .devices
            .into_iter()
            .map(|d| Device {
                key: d.key.to_string(),
                fingerprint: d.key.fingerprint(),
                role: d.role,
                introducer: d.introducer,
                display_name: d.display_name,
                platform: d.platform,
                revoked: d.revoked,
                connected: d.connected,
                this_device: d.this_device,
                principal: d.principal.map(|p| p.to_string()),
            })
            .collect())
    }

    /// Revokes a device key and pushes the revocation to connected peers.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::BadRequest`] for a malformed key,
    /// [`LpError::Mismatch`] if this device is not an introducer, or a store
    /// error.
    pub fn revoke(&self, key: String) -> Result<(), LpError> {
        let key = PublicKey::parse(key.trim())?;
        let daemon = self.daemon.clone();
        self.with_runtime(|rt| {
            rt.block_on(async move { ligatured::control::revoke_and_push(&daemon, &key).await })
        })??;
        self.host.on_event(Event::StatusChanged);
        Ok(())
    }

    /// The host read the system clipboard and found `text` on it: send it to
    /// the person's other devices unless it is what they just sent us.
    pub fn clipboard_changed(&self, text: String) {
        self.clipboard.changed(&text);
    }

    /// The host read the system clipboard and found a value with these
    /// representations (milestone 5): the text, the HTML, and the content
    /// the app opened for the node to read, each as a path with its name,
    /// size and type. An image up to the kept bound becomes a part; anything
    /// else becomes a file of the selection. Sent to the person's other
    /// devices unless it is what they just sent us.
    pub fn clipboard_changed_value(
        &self,
        text: Option<String>,
        html: Option<String>,
        contents: Vec<OutgoingFile>,
    ) {
        const KEEP: u64 = 64 * 1024 * 1024;
        let mut value = ligatured::clipboard::Value {
            text,
            parts: Vec::new(),
            files: Vec::new(),
        };
        if let Some(h) = html {
            value
                .parts
                .push(ligatured::clipboard::Part::new("text/html", h.into_bytes()));
        }
        for c in contents {
            let is_image = c.mime.to_ascii_lowercase().starts_with("image/");
            if is_image
                && c.size <= KEEP
                && !value
                    .parts
                    .iter()
                    .any(|p| p.mime.eq_ignore_ascii_case(&c.mime))
                && let Ok(bytes) = std::fs::read(&c.path)
            {
                value
                    .parts
                    .push(ligatured::clipboard::Part::new(&c.mime, bytes));
                continue;
            }
            value.files.push(ligatured::clipboard::LocalFile {
                path: PathBuf::from(&c.path),
                name: ligatured::files::safe_name(&c.name),
                size: c.size,
                mime: if c.mime.trim().is_empty() {
                    ligatured::files::mime_for(&c.name).to_owned()
                } else {
                    c.mime
                },
                modified: c.modified.and_then(|t| Timestamp::from_unix(t).ok()),
            });
        }
        if value.is_empty() {
            return;
        }
        self.clipboard.changed_value(value);
    }

    /// The last clipboard text the node set or was told about.
    #[must_use]
    pub fn clipboard_text(&self) -> Option<String> {
        self.clipboard.read_text().ok().flatten()
    }

    /// What the app's content provider serves for `key`: the bytes of a
    /// pushed part, or a promised part or file fetched now from the device
    /// that announced it. `None` when the key is gone, the fetch failed or
    /// took longer than [`CLIPBOARD_FETCH_TIMEOUT`], or the node has
    /// stopped. Called from a binder thread; other node calls are not held
    /// up while it waits.
    #[must_use]
    pub fn clipboard_item(&self, key: String) -> Option<crate::ClipItemData> {
        let clipboard = self.clipboard.clone();
        self.with_runtime(|rt| {
            rt.block_on(async move {
                match tokio::time::timeout(CLIPBOARD_FETCH_TIMEOUT, clipboard.item(&key)).await {
                    Ok(item) => item,
                    Err(_) => {
                        tracing::warn!(key, "clipboard item fetch timed out");
                        None
                    }
                }
            })
        })
        .ok()
        .flatten()
    }

    /// Asks every connected device that offers the clipboard for its current
    /// value (**Receive now**). Returns how many were asked.
    pub fn request_clipboard(&self) -> u32 {
        let Some(sync) = self.daemon.clipboard_sync() else {
            return 0;
        };
        let peers = self
            .daemon
            .registry()
            .peers_offering(lp_core::capability::CLIPBOARD_TYPE);
        self.with_runtime(|rt| {
            rt.block_on(async move {
                let mut asked = 0;
                for p in peers {
                    if sync.request(&p).await {
                        asked += 1;
                    }
                }
                asked
            })
        })
        .unwrap_or(0)
    }

    /// Fetches the files the clipboard currently promises into Downloads,
    /// as a transfer the host sees like any other (**Save**). Returns the
    /// transfer id, or `None` when nothing is promised.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::Internal`] if the device that promised them is
    /// not connected.
    pub fn save_clipboard_files(&self) -> Result<Option<String>, LpError> {
        let Some(sync) = self.daemon.clipboard_sync() else {
            return Ok(None);
        };
        let (Some(peer), files) = (sync.promised_from(), sync.promised_files()) else {
            return Ok(None);
        };
        if files.is_empty() {
            return Ok(None);
        }
        let ids: Vec<String> = files.into_iter().map(|f| f.id).collect();
        let transfer = self.with_runtime(|rt| {
            let _guard = rt.enter();
            self.files.fetch(&peer, ids, Destination::Downloads)
        })??;
        Ok(Some(transfer.id()))
    }

    /// The listener saw an app post or update `notification`: send it to
    /// every device of the person that displays notifications.
    pub fn notification_posted(&self, notification: crate::LocalNotification) {
        self.notifications.posted(notification);
    }

    /// One of this device's own notifications went away (the user dismissed
    /// it or the app withdrew it): its copies elsewhere go too.
    pub fn notification_removed(&self, id: String) {
        self.notifications.removed(id);
    }

    /// The user dismissed the copy of `origin`'s notification `id` on this
    /// device: the origin cancels its own and every other display clears.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::BadRequest`] if `origin` is not a key.
    pub fn notification_copy_dismissed(&self, origin: String, id: String) -> Result<(), LpError> {
        let origin = PublicKey::parse(origin.trim())?;
        self.notifications.copy_dismissed(origin, id);
        Ok(())
    }

    /// The notification listener connected (`true`) or disconnected: the
    /// capability's `emits` follows it and peers are told.
    pub fn set_notifications_emitting(&self, emitting: bool) {
        self.notifications.set_emits(emitting);
        if let Some(sync) = self.daemon.notification_sync() {
            let _ = self.with_runtime(|rt| rt.block_on(sync.advertise()));
        }
    }

    /// Starts the mDNS responder and browser again: for the host to call when
    /// the network came up or changed, since a responder started before Wi-Fi
    /// had an address may never announce or hear anything. Peers reached
    /// before are redialled by the node on its own.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::Internal`] if the responder cannot start.
    pub fn restart_discovery(&self) -> Result<(), LpError> {
        self.with_runtime(|rt| {
            let _guard = rt.enter();
            self.daemon.restart_discovery()
        })?
        .map_err(LpError::from)
    }

    /// Starts a ceremony with this device as the newcomer. The offer and the
    /// code arrive as events; the returned [`Pairing`] takes the answer.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::AlreadyInitialised`] or [`LpError::Mismatch`] if a
    /// ceremony is already waiting.
    pub fn start_join(self: &Arc<Self>) -> Result<Arc<Pairing>, LpError> {
        if self.daemon.is_initialised() {
            return Err(LpError::AlreadyInitialised);
        }
        let (prompt, pairing) = self.prompt();
        let (offer_tx, mut offer_rx) = mpsc::channel::<String>(1);
        let for_offer = pairing.clone();
        let host = self.host.clone();
        let daemon = self.daemon.clone();
        let finished = pairing.clone();
        let host_done = self.host.clone();
        self.with_runtime(|rt| {
            rt.spawn(async move {
                while let Some(offer) = offer_rx.recv().await {
                    for_offer.set_offer(offer.clone());
                    host.on_event(Event::PairingOffer { offer });
                }
            });
            rt.spawn(async move {
                let result = ceremony::join(&daemon, prompt, offer_tx)
                    .await
                    .map_err(|e| e.to_string());
                finish(&finished, &host_done, result);
            });
        })?;
        Ok(pairing)
    }

    /// Starts a ceremony with this device as the introducer of the newcomer
    /// that showed `offer`.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::NotInitialised`], [`LpError::BadRequest`] for a
    /// malformed offer, or [`LpError::Mismatch`] if this device is not an
    /// introducer.
    pub fn start_introduce(
        self: &Arc<Self>,
        offer: String,
        make_introducer: bool,
    ) -> Result<Arc<Pairing>, LpError> {
        if !self.daemon.is_initialised() {
            return Err(LpError::NotInitialised);
        }
        lp_identity::pairing::Offer::parse(offer.trim())?;
        if self.daemon.self_signing()?.is_none() {
            return Err(LpError::Mismatch(
                "this device is not an introducer".to_owned(),
            ));
        }
        let (prompt, pairing) = self.prompt();
        let daemon = self.daemon.clone();
        let finished = pairing.clone();
        let host = self.host.clone();
        let text = offer.trim().to_owned();
        self.with_runtime(|rt| {
            rt.spawn(async move {
                let result = ceremony::introduce(&daemon, &text, make_introducer, prompt)
                    .await
                    .map_err(|e| e.to_string());
                finish(&finished, &host, result);
            });
        })?;
        Ok(pairing)
    }

    /// Stops advertising, closes every connection and ends the runtime.
    /// The node is unusable afterwards; the host starts a new one.
    pub fn stop(&self) {
        self.daemon.shutdown();
        if let Some(rt) = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            rt.shutdown_timeout(Duration::from_secs(2));
        }
        logging::clear(&self.host);
        tracing::info!("node stopped");
    }
}

impl Node {
    /// The daemon, for tests.
    #[must_use]
    pub fn daemon(&self) -> &Arc<Daemon> {
        &self.daemon
    }

    fn watching_slot(&self) -> std::sync::Weak<Mutex<WatchSlot>> {
        Arc::downgrade(&self.watching)
    }

    fn input_sync(&self) -> Result<Arc<ligatured::input::InputSync>, LpError> {
        self.daemon
            .input_sync()
            .ok_or_else(|| LpError::Internal("input is not attached".to_owned()))
    }

    fn prompt(&self) -> (Prompt, Arc<Pairing>) {
        let (confirm_tx, confirm_rx) = oneshot::channel();
        let (code_tx, mut code_rx) = mpsc::channel::<String>(1);
        let pairing = Arc::new(Pairing::new(confirm_tx));
        let host = self.host.clone();
        self.with_runtime(|rt| {
            rt.spawn(async move {
                while let Some(code) = code_rx.recv().await {
                    host.on_event(Event::PairingCode { code });
                }
            });
        })
        .ok();
        (
            Prompt {
                code: code_tx,
                confirm: confirm_rx,
            },
            pairing,
        )
    }

    /// Runs `f` with a handle to the node's runtime. The lock protecting the
    /// runtime is held only to take the handle, never while `f` waits on the
    /// network, so one call blocking on a fetch stalls no other (LP-127).
    ///
    /// # Errors
    ///
    /// Returns [`LpError::Internal`] once [`Self::stop`] has taken the
    /// runtime down: a provider or activity call that lands then gets an
    /// error, not a panic.
    fn with_runtime<R>(&self, f: impl FnOnce(&tokio::runtime::Handle) -> R) -> Result<R, LpError> {
        let handle = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(tokio::runtime::Runtime::handle)
            .cloned()
            .ok_or_else(|| LpError::Internal("node stopped".to_owned()))?;
        Ok(f(&handle))
    }
}

type WatchSlot = Option<(
    PublicKey,
    tokio::sync::mpsc::UnboundedSender<ligatured::screen::ShareControl>,
)>;

fn lock_watch(m: &Mutex<WatchSlot>) -> std::sync::MutexGuard<'_, WatchSlot> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn finish(pairing: &Pairing, host: &SharedHost, result: Result<(), String>) {
    pairing.finish(result.clone());
    host.on_event(Event::PairingFinished {
        error: result.err(),
    });
    host.on_event(Event::StatusChanged);
}
