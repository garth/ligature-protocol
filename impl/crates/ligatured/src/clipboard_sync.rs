//! Keeps the clipboard in step across a person's devices. Spec chapter 05
//! section 3b; milestone 1 tasks E5 and E6, milestone 5 LP-107.
//!
//! A local change becomes a `set` on the clipboard stream to every connected
//! binding of the same person that offers the capability, built per
//! receiver: the text and the parts that fit its push bound are pushed, the
//! rest announced as `files` offers it can fetch when an application pastes.
//! A `set` received from such a binding is applied if it is newer than the
//! last one applied and not an echo of our own, its announced parts held as
//! promises on the local clipboard. A `get` asks a peer for its current
//! value, sent on connecting. The capability is advertised only while the
//! local clipboard is reachable.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use lp_core::capability::{
    CLIPBOARD_GET_TYPE, CLIPBOARD_SET_TYPE, CLIPBOARD_TYPE, ClipboardGet, ClipboardMessage,
    ClipboardParams, ClipboardPart, ClipboardSet, clipboard_descriptor_with,
};
use lp_core::files::FileDescriptor;
use lp_core::wire::{Bytes, Timestamp};
use lp_core::{Error, PublicKey};
use lp_crypto::hash::sha256;
use lp_identity::trust::Recognition;
use lp_transport::manager::{StreamHandler, open_on};
use lp_transport::quic::Connection;
use tokio::sync::mpsc;

use crate::clipboard::{
    Clipboard, EchoFilter, Fetch, FetchFuture, Fetched, Fingerprint, Incoming, LocalFile, Part,
    Promise, TEXT_PLAIN, Value, pushed_part,
};
use crate::daemon::Daemon;
use crate::files::{Destination, FileSource};
use crate::files_sync::FilesSync;

/// The largest text accepted in one `set`, in UTF-8 bytes: the frame limit
/// less headroom for the rest of the message.
pub const MAX_TEXT_BYTES: u64 = 1024 * 1024 - 1024;

/// How often availability of the local clipboard is re-checked.
const AVAILABILITY_POLL: Duration = Duration::from_secs(1);

/// How the daemon takes part in the clipboard (plan decisions 3, 7 and 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardConfig {
    /// The push bound this device advertises as `maxBytes`.
    pub max_bytes: u64,
    /// Bytes kept per clipboard value for announcing; parts beyond are
    /// dropped from the announcement, largest first.
    pub keep_bytes: u64,
    /// A fetched part up to this size stays in memory; larger ones and
    /// files land in the cache directory.
    pub memory_limit: u64,
    /// How long announced content stays offered when the clipboard does
    /// not change first.
    pub window: Duration,
    /// Where fetched parts and files live before a paste completes.
    pub cache_dir: PathBuf,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            max_bytes: ClipboardParams::DEFAULT_MAX_BYTES,
            keep_bytes: 64 * 1024 * 1024,
            memory_limit: 16 * 1024 * 1024,
            window: Duration::from_secs(60 * 60),
            cache_dir: crate::clipboard::platform_cache_dir(),
        }
    }
}

/// The value the local clipboard holds as the daemon knows it: a user's
/// copy or what a peer sent, with what this device offers for it.
struct LocalValue {
    value: Arc<Value>,
    set_at: Timestamp,
    generation: u64,
    /// Offers registered for announcing, by part type or `file:<index>`.
    offers: HashMap<String, FileDescriptor>,
}

/// The value applied from a peer, with its promises, while it stands.
struct Applied {
    peer: PublicKey,
    incoming: Incoming,
}

/// The clipboard synchroniser for one daemon.
pub struct ClipboardSync {
    daemon: Arc<Daemon>,
    clipboard: Arc<dyn Clipboard>,
    config: ClipboardConfig,
    filter: EchoFilter,
    latest: Mutex<Option<Timestamp>>,
    local: Mutex<Option<LocalValue>>,
    applied_value: Mutex<Option<Applied>>,
    generation: AtomicU64,
    senders: Mutex<HashMap<PublicKey, mpsc::Sender<ClipboardMessage>>>,
    sent: Mutex<u64>,
    applied: Mutex<u64>,
}

impl std::fmt::Debug for ClipboardSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClipboardSync")
    }
}

impl ClipboardSync {
    /// Starts synchronising `clipboard` for `daemon` with the default
    /// configuration.
    #[must_use]
    pub fn start(daemon: Arc<Daemon>, clipboard: Arc<dyn Clipboard>) -> Arc<Self> {
        Self::start_with(daemon, clipboard, ClipboardConfig::default())
    }

    /// Starts synchronising `clipboard` for `daemon`: registers the stream
    /// handler, watches local changes and availability, clears the cache.
    #[must_use]
    pub fn start_with(
        daemon: Arc<Daemon>,
        clipboard: Arc<dyn Clipboard>,
        config: ClipboardConfig,
    ) -> Arc<Self> {
        let _ = std::fs::remove_dir_all(&config.cache_dir);
        let this = Arc::new(Self {
            daemon: daemon.clone(),
            clipboard,
            config,
            filter: EchoFilter::default(),
            latest: Mutex::new(None),
            local: Mutex::new(None),
            applied_value: Mutex::new(None),
            generation: AtomicU64::new(0),
            senders: Mutex::new(HashMap::new()),
            sent: Mutex::new(0),
            applied: Mutex::new(0),
        });
        daemon.manager().on_stream(CLIPBOARD_TYPE, this.handler());
        // Subscribe before anything else runs, so a copy made right after
        // attaching is not missed, and start from what the clipboard holds.
        let changes = this.clipboard.subscribe();
        if let Ok(Some(value)) = this.clipboard.read_value()
            && !value.is_empty()
        {
            let fingerprint = value.fingerprint();
            if this.filter.should_send(&fingerprint) {
                let set_at = Timestamp::now();
                this.replace_local(Arc::new(value), set_at);
                *lock(&this.latest) = Some(set_at);
            }
        }
        let local = this.clone();
        tokio::spawn(async move { local.watch_local(changes).await });
        let avail = this.clone();
        tokio::spawn(async move { avail.watch_availability().await });
        this
    }

    /// How many `set` messages went out and how many were applied, for tests.
    #[must_use]
    pub fn counters(&self) -> (u64, u64) {
        (*lock(&self.sent), *lock(&self.applied))
    }

    /// The params this device advertises.
    #[must_use]
    pub fn params(&self) -> ClipboardParams {
        ClipboardParams {
            max_bytes: self.config.max_bytes.min(ClipboardParams::MAX_MAX_BYTES),
            mime_types: self.clipboard.mime_types(),
            fetch: self.clipboard.holds_promises() && self.daemon.files_sync().is_some(),
            rich: true,
        }
    }

    /// The promises the local clipboard currently holds, for `lpctl`.
    #[must_use]
    pub fn promises(&self) -> Vec<Promise> {
        lock(&self.applied_value)
            .as_ref()
            .map(|a| a.incoming.promises.clone())
            .unwrap_or_default()
    }

    /// The peer whose promises the local clipboard currently holds.
    #[must_use]
    pub fn promised_from(&self) -> Option<PublicKey> {
        lock(&self.applied_value).as_ref().map(|a| a.peer)
    }

    /// The files the local clipboard currently promises, for `lpctl`.
    #[must_use]
    pub fn promised_files(&self) -> Vec<FileDescriptor> {
        lock(&self.applied_value)
            .as_ref()
            .map(|a| a.incoming.files.clone())
            .unwrap_or_default()
    }

    /// A trusted peer connected: open our clipboard stream to it if it is a
    /// binding of the same person, and ask for its current value.
    pub fn attach(self: &Arc<Self>, conn: &Connection) {
        if !self.same_person(conn.peer()) {
            return;
        }
        let peer = *conn.peer();
        let (tx, mut rx) = mpsc::channel::<ClipboardMessage>(8);
        lock(&self.senders).insert(peer, tx);
        let conn = conn.clone();
        let sync = self.clone();
        tokio::spawn(async move {
            let mut stream = match open_on(&conn, CLIPBOARD_TYPE).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(%peer, %e, "cannot open clipboard stream");
                    return;
                }
            };
            // Ask for the peer's value, once its manifest says it understands
            // a get: a milestone 2 peer would end the stream on one.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            let mut params = sync.peer_params(&peer);
            while params.is_none() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(25)).await;
                params = sync.peer_params(&peer);
            }
            if params.is_some_and(|p| p.rich)
                && stream
                    .send(&ClipboardMessage::Get(ClipboardGet::default()))
                    .await
                    .is_err()
            {
                return;
            }
            while let Some(message) = rx.recv().await {
                if stream.send(&message).await.is_err() {
                    break;
                }
            }
            stream.finish();
        });
    }

    /// Asks `peer` for its current value, as on connecting; the answer is
    /// applied like any `set`. Returns whether the peer is reachable.
    pub async fn request(&self, peer: &PublicKey) -> bool {
        if !self.peer_params(peer).is_some_and(|p| p.rich) {
            return false;
        }
        let tx = lock(&self.senders).get(peer).cloned();
        match tx {
            Some(tx) => tx
                .send(ClipboardMessage::Get(ClipboardGet::default()))
                .await
                .is_ok(),
            None => false,
        }
    }

    /// A peer disconnected: its promises can no longer be kept.
    pub fn detach(&self, peer: &PublicKey) {
        lock(&self.senders).remove(peer);
        self.drop_promises_from(peer, "the peer disconnected");
    }

    fn same_person(&self, peer: &PublicKey) -> bool {
        matches!(
            self.daemon.recognise(peer),
            Ok(Recognition::MyBinding { .. })
        )
    }

    fn peer_params(&self, peer: &PublicKey) -> Option<ClipboardParams> {
        self.daemon
            .registry()
            .peers()
            .into_iter()
            .find(|c| c.peer == *peer)
            .and_then(|c| {
                c.capabilities
                    .iter()
                    .find_map(ClipboardParams::from_descriptor)
            })
    }

    async fn watch_local(
        self: Arc<Self>,
        mut changes: tokio::sync::broadcast::Receiver<crate::clipboard::Change>,
    ) {
        loop {
            let change = match changes.recv().await {
                Ok(c) => c,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            if change.value.is_empty() {
                continue;
            }
            if change
                .value
                .text
                .as_ref()
                .is_some_and(|t| t.len() as u64 > MAX_TEXT_BYTES)
            {
                tracing::debug!("clipboard text too large to send");
                continue;
            }
            let fingerprint = change.value.fingerprint();
            if !self.filter.should_send(&fingerprint) {
                continue;
            }
            let set_at = Timestamp::now();
            let generation = self.replace_local(Arc::new(change.value), set_at);
            *lock(&self.latest) = Some(set_at);
            self.send_to_all().await;
            self.schedule_withdrawal(generation);
        }
    }

    /// Makes `value` the local value: withdraws the previous one's offers,
    /// drops the promises of a value a peer sent, clears the cache.
    fn replace_local(&self, value: Arc<Value>, set_at: Timestamp) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let previous = lock(&self.local).replace(LocalValue {
            value,
            set_at,
            generation,
            offers: HashMap::new(),
        });
        if let Some(p) = previous {
            self.withdraw_offers(&p.offers);
        }
        lock(&self.applied_value).take();
        self.clear_cache(generation);
        generation
    }

    fn withdraw_offers(&self, offers: &HashMap<String, FileDescriptor>) {
        if offers.is_empty() {
            return;
        }
        if let Some(files) = self.daemon.files_sync() {
            for f in offers.values() {
                files.withdraw(&f.id);
            }
        }
    }

    fn schedule_withdrawal(self: &Arc<Self>, generation: u64) {
        let sync = self.clone();
        let window = self.config.window;
        tokio::spawn(async move {
            tokio::time::sleep(window).await;
            let offers = {
                let mut local = lock(&sync.local);
                match local.as_mut() {
                    Some(l) if l.generation == generation => std::mem::take(&mut l.offers),
                    _ => return,
                }
            };
            if !offers.is_empty() {
                tracing::debug!("clipboard offers withdrawn after the window");
                sync.withdraw_offers(&offers);
            }
        });
    }

    /// Sends the current local value to every connected binding that
    /// offers the clipboard, built for each.
    async fn send_to_all(&self) {
        let targets: Vec<(PublicKey, mpsc::Sender<ClipboardMessage>)> = {
            let offering = self.daemon.registry().peers_offering(CLIPBOARD_TYPE);
            lock(&self.senders)
                .iter()
                .filter(|(p, _)| offering.contains(p))
                .map(|(p, s)| (*p, s.clone()))
                .collect()
        };
        for (peer, tx) in targets {
            self.send_current_to(&peer, &tx).await;
        }
    }

    async fn send_current_to(&self, peer: &PublicKey, tx: &mpsc::Sender<ClipboardMessage>) {
        if !self.same_person(peer) {
            return;
        }
        let Some(params) = self.peer_params(peer) else {
            return;
        };
        let Some(set) = self.build_for(&params) else {
            return;
        };
        if !params.rich && set.text.is_none() {
            // A milestone 2 peer decodes only a set with text.
            return;
        }
        if tx.send(ClipboardMessage::Set(set)).await.is_ok() {
            *lock(&self.sent) += 1;
            tracing::debug!(%peer, "clipboard sent");
        }
    }

    /// The `set` for a receiver with `params` from the current local value,
    /// registering offers for what is announced. `None` with no local value.
    fn build_for(&self, params: &ClipboardParams) -> Option<ClipboardSet> {
        let files_sync = self.daemon.files_sync();
        let fetch = params.fetch && files_sync.is_some();
        let mut local = lock(&self.local);
        let local = local.as_mut()?;
        let value = local.value.clone();
        let mut set = ClipboardSet::empty();
        set.set_at = local.set_at;
        let mut budget = params.max_bytes;
        // Text travels as text, within the bound; a larger text is announced.
        let mut announce: Vec<(String, &Part)> = Vec::new();
        let text_part;
        if let Some(t) = &value.text {
            if t.len() as u64 <= budget {
                budget -= t.len() as u64;
                set.text = Some(t.clone());
            } else if fetch {
                text_part = Part::new(TEXT_PLAIN, t.clone().into_bytes());
                announce.push((TEXT_PLAIN.to_owned(), &text_part));
            }
        }
        let mut parts: Vec<&Part> = value
            .parts
            .iter()
            .filter(|p| params.accepts(&p.mime) && !p.mime.eq_ignore_ascii_case(TEXT_PLAIN))
            .collect();
        parts.sort_by_key(|p| std::cmp::Reverse(p.bytes.len()));
        for p in parts {
            let size = p.bytes.len() as u64;
            if size <= budget {
                budget -= size;
                set.parts.push(pushed_part(p));
            } else if fetch {
                announce.push((p.mime.clone(), p));
            }
        }
        // Keep the announced bytes within the bound, dropping the largest.
        announce.sort_by_key(|(_, p)| p.bytes.len());
        while announce
            .iter()
            .map(|(_, p)| p.bytes.len() as u64)
            .sum::<u64>()
            > self.config.keep_bytes
        {
            announce.pop();
        }
        if let Some(files) = &files_sync {
            for (mime, p) in announce {
                let key = mime.to_ascii_lowercase();
                let descriptor = match local.offers.get(&key) {
                    Some(d) => d.clone(),
                    None => {
                        let Ok(id) = FilesSync::new_id() else {
                            continue;
                        };
                        let mut d =
                            FileDescriptor::new(&id, &part_name(&mime), p.bytes.len() as u64);
                        d.mime = mime.clone();
                        d.sha256 = Some(Bytes(sha256(&p.bytes).to_vec()).to_base64url());
                        files.offer(d.clone(), FileSource::Bytes(p.bytes.clone()));
                        local.offers.insert(key, d.clone());
                        d
                    }
                };
                set.parts.push(ClipboardPart::announced(&mime, descriptor));
            }
            if fetch {
                for (i, f) in value.files.iter().enumerate() {
                    let key = format!("file:{i}");
                    let descriptor = match local.offers.get(&key) {
                        Some(d) => d.clone(),
                        None => {
                            let Ok(id) = FilesSync::new_id() else {
                                continue;
                            };
                            let mut d = FileDescriptor::new(&id, &f.name, f.size);
                            d.mime = f.mime.clone();
                            d.modified = f.modified;
                            files.offer(d.clone(), FileSource::Path(f.path.clone()));
                            local.offers.insert(key, d.clone());
                            d
                        }
                    };
                    set.files.push(descriptor);
                }
            }
        }
        Some(set)
    }

    async fn watch_availability(self: Arc<Self>) {
        let mut offered: Option<ClipboardParams> = None;
        loop {
            let available = self.clipboard.available();
            let params = self.params();
            // Offered on becoming available, and again when the params
            // change, as when files are attached after the clipboard.
            if available && offered.as_ref() != Some(&params) {
                self.daemon
                    .registry()
                    .offer(clipboard_descriptor_with(&params))
                    .await;
                tracing::info!(fetch = params.fetch, "clipboard capability offered");
                offered = Some(params);
            } else if !available && offered.is_some() {
                self.daemon.registry().withdraw(CLIPBOARD_TYPE).await;
                offered = None;
                tracing::info!("clipboard capability withdrawn");
            }
            tokio::time::sleep(AVAILABILITY_POLL).await;
        }
    }

    /// Applies a `set` from `peer` if it is newer than what we hold and not an
    /// echo. Returns whether the local clipboard changed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] if the peer is not a binding of this person
    /// and [`Error::Io`] if the clipboard cannot be set.
    pub fn apply(self: &Arc<Self>, peer: &PublicKey, set: &ClipboardSet) -> Result<bool, Error> {
        if set.message_type.as_str() != CLIPBOARD_SET_TYPE {
            return Err(Error::Decode(format!(
                "expected a clipboard set, got {}",
                set.message_type
            )));
        }
        if !self.same_person(peer) {
            return Err(Error::Chain(
                "clipboard set from a device that is not mine".to_owned(),
            ));
        }
        if set
            .text
            .as_ref()
            .is_some_and(|t| t.len() as u64 > MAX_TEXT_BYTES)
        {
            return Err(Error::Decode("clipboard text too large".to_owned()));
        }
        if set.is_empty() {
            // An empty answer to a get, or a cleared clipboard: nothing to
            // put on ours.
            return Ok(false);
        }
        {
            let latest = lock(&self.latest);
            if latest.is_some_and(|l| set.set_at < l) {
                return Ok(false);
            }
        }
        let fingerprint = Fingerprint::of_set(set);
        if !self.filter.should_apply(&fingerprint) {
            return Ok(false);
        }
        if lock(&self.local)
            .as_ref()
            .is_some_and(|l| l.value.fingerprint() == fingerprint)
        {
            *lock(&self.latest) = Some(set.set_at);
            return Ok(false);
        }
        let generation = self.generation.load(Ordering::SeqCst) + 1;
        let fetcher: Arc<dyn Fetch> = Arc::new(Fetcher {
            sync: Arc::downgrade(self),
            peer: *peer,
            generation,
            sizes: set
                .parts
                .iter()
                .filter_map(|p| p.file.as_ref())
                .chain(set.files.iter())
                .map(|f| (f.id.clone(), f.clone()))
                .collect(),
            files: set.files.iter().map(|f| f.id.clone()).collect(),
            cache: Mutex::new(HashMap::new()),
        });
        let mut incoming = Incoming {
            text: set.text.clone(),
            parts: Vec::new(),
            promises: Vec::new(),
            files: set.files.clone(),
            fetch: Some(fetcher),
        };
        for p in &set.parts {
            match (&p.bytes, &p.file) {
                (Some(b), _) => incoming.parts.push(Part::new(&p.mime, b.0.clone())),
                (None, Some(f)) => incoming.promises.push(Promise {
                    mime: p.mime.clone(),
                    file: f.clone(),
                }),
                (None, None) => {}
            }
        }
        self.filter.note_set(&fingerprint);
        let read_back = Value {
            text: incoming.text.clone(),
            parts: incoming.parts.clone(),
            files: Vec::new(),
        };
        // Record the value as applied before the platform gets it: a paste,
        // or a provider asked the moment the clipboard is set, looks the
        // promises up here, and must not find the previous value (LP-107).
        let g = self.replace_local(Arc::new(read_back), set.set_at);
        debug_assert_eq!(g, generation);
        let has_promises = incoming.has_promises();
        if has_promises {
            *lock(&self.applied_value) = Some(Applied {
                peer: *peer,
                incoming: incoming.clone(),
            });
        }
        if let Err(e) = self.clipboard.set_value(&incoming) {
            if has_promises {
                lock(&self.applied_value).take();
            }
            return Err(e);
        }
        *lock(&self.latest) = Some(set.set_at);
        *lock(&self.applied) += 1;
        crate::grants::record_implicit(&self.daemon, peer);
        Ok(true)
    }

    /// The value a peer sent can no longer be served by it: keep what was
    /// pushed, drop the promises.
    fn drop_promises_from(&self, peer: &PublicKey, why: &str) {
        let applied = {
            let mut slot = lock(&self.applied_value);
            match slot.as_ref() {
                Some(a) if a.peer == *peer => slot.take(),
                _ => None,
            }
        };
        let Some(applied) = applied else { return };
        tracing::info!(%peer, why, "clipboard promises dropped");
        let kept = applied.incoming.without_promises();
        let read_back = Value {
            text: kept.text.clone(),
            parts: kept.parts.clone(),
            files: Vec::new(),
        };
        self.filter.note_set(&read_back.fingerprint());
        if let Err(e) = self.clipboard.set_value(&kept) {
            tracing::debug!(%e, "cannot reset the clipboard after dropping promises");
        }
    }

    /// Removes every cached generation but `keep`.
    fn clear_cache(&self, keep: u64) {
        let Ok(entries) = std::fs::read_dir(&self.config.cache_dir) else {
            return;
        };
        for e in entries.flatten() {
            if e.file_name().to_string_lossy() != keep.to_string() {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    /// Answers a `get` from `peer` with the current value built for it, or
    /// an empty `set`.
    async fn answer_get(&self, peer: &PublicKey) {
        // A get arrives right after connecting, often before the asker's
        // manifest and before our own stream to it is open; wait a moment
        // for both rather than answer blind or not at all.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let (tx, params) = loop {
            let tx = lock(&self.senders).get(peer).cloned();
            let params = self.peer_params(peer);
            match (tx, params) {
                (Some(tx), Some(params)) => break (tx, Some(params)),
                (Some(tx), None) if tokio::time::Instant::now() >= deadline => break (tx, None),
                (None, _) if tokio::time::Instant::now() >= deadline => return,
                _ => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        };
        let set = params
            .and_then(|params| self.build_for(&params))
            .unwrap_or_else(ClipboardSet::empty);
        let counts = !set.is_empty();
        if tx.send(ClipboardMessage::Set(set)).await.is_ok() {
            if counts {
                *lock(&self.sent) += 1;
            }
            tracing::debug!(%peer, "clipboard get answered");
        }
    }

    fn handler(self: &Arc<Self>) -> StreamHandler {
        let sync = self.clone();
        Arc::new(move |conn, mut stream| {
            let sync = sync.clone();
            Box::pin(async move {
                let peer = *conn.peer();
                loop {
                    match stream.recv::<ClipboardMessage>().await {
                        Ok(Some(ClipboardMessage::Set(set))) => match sync.apply(&peer, &set) {
                            Ok(true) => tracing::debug!(%peer, "clipboard applied"),
                            Ok(false) => {}
                            Err(e) => {
                                tracing::warn!(%peer, %e, "clipboard set refused");
                                stream.reset(lp_core::frame::REFUSED_CODE);
                                break;
                            }
                        },
                        Ok(Some(ClipboardMessage::Get(get))) => {
                            if get.message_type.as_str() == CLIPBOARD_GET_TYPE
                                && sync.same_person(&peer)
                            {
                                sync.answer_get(&peer).await;
                            } else {
                                tracing::debug!(%peer, "clipboard message ignored");
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            tracing::debug!(%peer, %e, "clipboard stream ended");
                            break;
                        }
                    }
                }
            })
        })
    }
}

/// Fetches the promises of one applied value from the peer that sent it.
struct Fetcher {
    sync: Weak<ClipboardSync>,
    peer: PublicKey,
    generation: u64,
    sizes: HashMap<String, FileDescriptor>,
    files: Vec<String>,
    cache: Mutex<HashMap<String, Fetched>>,
}

impl Fetcher {
    async fn fetch_id(&self, id: &str) -> Result<Fetched, Error> {
        if let Some(f) = lock(&self.cache).get(id) {
            return Ok(f.clone());
        }
        let sync = self
            .sync
            .upgrade()
            .ok_or_else(|| Error::Io("the daemon is shutting down".to_owned()))?;
        let files = sync
            .daemon
            .files_sync()
            .ok_or_else(|| Error::Io("no files capability to fetch with".to_owned()))?;
        let descriptor = self
            .sizes
            .get(id)
            .ok_or_else(|| Error::Io(format!("{id} was not announced")))?;
        let to_disk =
            self.files.iter().any(|f| f == id) || descriptor.size > sync.config.memory_limit;
        let destination = if to_disk {
            let dir = sync.config.cache_dir.join(self.generation.to_string());
            std::fs::create_dir_all(&dir)
                .map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
            Destination::Path(dir)
        } else {
            Destination::Memory
        };
        let result = match files.fetch(&self.peer, vec![id.to_owned()], destination) {
            Ok(transfer) => {
                let progress = transfer.wait().await;
                if progress.all_received() {
                    transfer
                        .take_landed(id)
                        .ok_or_else(|| Error::Io("nothing landed".to_owned()))
                } else {
                    Err(Error::Io(format!(
                        "fetch of {id} ended: {:?}",
                        progress.finished
                    )))
                }
            }
            Err(e) => Err(e),
        };
        let landed = match result {
            Ok(l) => l,
            Err(e) => {
                sync.drop_promises_from(&self.peer, &e.to_string());
                return Err(e);
            }
        };
        if let Some(expected) = &descriptor.sha256
            && *expected != landed.sha256
        {
            sync.drop_promises_from(
                &self.peer,
                "the fetched bytes do not match the announcement",
            );
            return Err(Error::Io(format!("{id}: hash mismatch")));
        }
        let fetched = Fetched {
            bytes: landed.bytes.map(Arc::new),
            path: landed.path,
        };
        lock(&self.cache).insert(id.to_owned(), fetched.clone());
        Ok(fetched)
    }
}

impl Fetch for Fetcher {
    fn fetch(&self, id: &str) -> FetchFuture<'_> {
        let id = id.to_owned();
        Box::pin(async move { self.fetch_id(&id).await })
    }
}

/// The name an announced part is offered under: `clipboard` with the
/// type's usual extension.
fn part_name(mime: &str) -> String {
    let ext = match mime.to_ascii_lowercase().as_str() {
        "text/plain" => "txt",
        "text/html" => "html",
        "text/rtf" | "application/rtf" => "rtf",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/tiff" => "tiff",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "text/uri-list" => "uris",
        _ => "bin",
    };
    format!("clipboard.{ext}")
}

/// A local file for a value, from its path, for callers that hand the
/// daemon a file selection.
///
/// # Errors
///
/// Returns [`Error::Io`] if the path is not a readable regular file.
pub fn local_file(path: &std::path::Path) -> Result<LocalFile, Error> {
    let (descriptor, _) = FileSource::describe_path("local", path)?;
    Ok(LocalFile {
        path: path.to_path_buf(),
        name: descriptor.name,
        size: descriptor.size,
        mime: descriptor.mime,
        modified: descriptor.modified,
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
