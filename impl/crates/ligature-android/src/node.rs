//! The node: a daemon on its own runtime, driven by the host's calls.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
}

/// A running daemon. One per process at a time; [`Node::stop`] ends it.
#[derive(uniffi::Object)]
pub struct Node {
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    daemon: Arc<Daemon>,
    clipboard: Arc<HostClipboard>,
    host: SharedHost,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Node({})", self.daemon.binding().public())
    }
}

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
        let node = Arc::new(Self {
            runtime: Mutex::new(Some(runtime)),
            daemon,
            clipboard: clipboard.clone(),
            host,
        });
        node.with_runtime(|rt| {
            let _guard = rt.enter();
            node.daemon.attach_clipboard(clipboard);
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
                    let now = daemon
                        .registry()
                        .peers_offering(lp_core::capability::CLIPBOARD_TYPE);
                    if now != last {
                        last = now;
                        host.on_event(Event::StatusChanged);
                    }
                }
            });
        });
        Ok(node)
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
        }
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
        })?;
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
        })?;
        self.host.on_event(Event::StatusChanged);
        Ok(())
    }

    /// The host read the system clipboard and found `text` on it: send it to
    /// the person's other devices unless it is what they just sent us.
    pub fn clipboard_changed(&self, text: String) {
        self.clipboard.changed(&text);
    }

    /// The last clipboard text the node set or was told about.
    #[must_use]
    pub fn clipboard_text(&self) -> Option<String> {
        self.clipboard.read_text().ok().flatten()
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
        })
        .map_err(LpError::from)
    }

    /// Whether the host can set the clipboard right now. While it cannot,
    /// the capability is withdrawn from peers.
    pub fn set_clipboard_reachable(&self, reachable: bool) {
        self.clipboard.set_reachable(reachable);
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
        });
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
        });
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
        });
        (
            Prompt {
                code: code_tx,
                confirm: confirm_rx,
            },
            pairing,
        )
    }

    fn with_runtime<R>(&self, f: impl FnOnce(&tokio::runtime::Runtime) -> R) -> R {
        let guard = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let rt = guard.as_ref().expect("node stopped");
        f(rt)
    }
}

fn finish(pairing: &Pairing, host: &SharedHost, result: Result<(), String>) {
    pairing.finish(result.clone());
    host.on_event(Event::PairingFinished {
        error: result.err(),
    });
    host.on_event(Event::StatusChanged);
}
