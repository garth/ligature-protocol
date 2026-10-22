//! Moves files between a person's devices. Spec chapter 05 section 3d;
//! milestone 4 (LP-102).
//!
//! A transfer is a control stream and a data stream per file, all with the
//! capability type as their purpose. The sender opens the control stream for
//! a push and sends an `offer`; the receiver opens it for a fetch and sends a
//! `request`, answered by an `offer` of what the sender still holds. The
//! receiver answers with an `accept`, and for each accepted file the sender
//! opens a data stream (`begin`, chunks, `end` with the hash) and reads the
//! receiver's `received` or `failed` on it. A sender keeps no more data
//! streams open to a peer than the smaller of the peer's `maxStreams` and its
//! own; data streams run below the priority of every other stream. Either
//! side cancels a file by resetting its data stream and a transfer by
//! resetting the control stream. Offers and transfer state live in memory.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lp_core::files::{
    CANCELLED_CODE, FAILED_CODE, FILES_TYPE, FileDescriptor, FilesAccept, FilesBegin, FilesChunk,
    FilesDecline, FilesEnd, FilesFailed, FilesMessage, FilesOffer, FilesParams, FilesRequest,
    Reason, files_descriptor,
};
use lp_core::wire::{Bytes, Timestamp};
use lp_core::{Error, PublicKey};
use lp_crypto::hash::Sha256Stream;
use lp_identity::trust::Recognition;
use lp_transport::manager::{StreamHandler, open_on};
use lp_transport::quic::Connection;
use lp_transport::stream::{FramedStream, peer_stop_code};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt as _;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tokio::task::JoinSet;

use crate::daemon::Daemon;
use crate::files::{Destination, DirectoryStore, FileSource, FileStore, Landing, MemoryLanding};

/// How the daemon takes part in file transfers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilesConfig {
    /// Data streams accepted at once from one peer: the advertised `maxStreams`.
    pub max_streams: u32,
    /// The largest chunk accepted: the advertised `maxChunkBytes`.
    pub max_chunk_bytes: u64,
    /// Data streams this device is willing to run at once towards one peer.
    pub send_streams: u32,
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            max_streams: 4,
            max_chunk_bytes: FilesParams::DEFAULT_MAX_CHUNK_BYTES,
            send_streams: 4,
        }
    }
}

/// How often the store's availability is re-read.
const ADVERTISE_POLL: Duration = Duration::from_secs(2);
/// How long to wait for the peer to finish its side of a control stream
/// after ours is finished.
const FINISH_WAIT: Duration = Duration::from_secs(10);
/// How many transfers one peer may have pending at once before `busy`.
const MAX_PENDING_PER_PEER: usize = 16;
/// The app id of the received-file notification.
pub const FILES_APP: &str = "org.ligatureprotocol.files";

/// How far one file has got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum FileState {
    /// Not started.
    Pending,
    /// Bytes are moving.
    Active,
    /// Verified and in place.
    Received {
        /// Where it landed, for display.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        location: Option<String>,
    },
    /// Not kept.
    Failed {
        /// The protocol's reason, when the other side gave one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<Reason>,
        /// What happened, for a person.
        message: String,
    },
    /// Stopped by a cancel.
    Cancelled,
}

/// One file of a transfer as it progresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileProgress {
    /// The file id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its size.
    pub size: u64,
    /// Bytes sent or received so far.
    pub done: u64,
    /// Where it is.
    pub state: FileState,
}

/// How a transfer ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum TransferEnd {
    /// Every accepted file was answered; see each file's state.
    Complete,
    /// The other side, or this one, declined before any file moved.
    Declined {
        /// Why.
        reason: Reason,
    },
    /// Cancelled from either end.
    Cancelled,
    /// The connection or the protocol failed.
    Failed {
        /// What happened.
        message: String,
    },
}

/// A transfer as it progresses: what `lpctl` shows and tests check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    /// The transfer id.
    pub transfer: String,
    /// The other device.
    pub peer: PublicKey,
    /// Whether this device sends the bytes.
    pub outgoing: bool,
    /// The files, in the offer's order.
    pub files: Vec<FileProgress>,
    /// Set once the transfer is over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<TransferEnd>,
}

impl TransferProgress {
    /// Bytes moved so far, over every file.
    #[must_use]
    pub fn done_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.done).sum()
    }

    /// Bytes to move, over every file.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Whether the transfer completed with every file received.
    #[must_use]
    pub fn all_received(&self) -> bool {
        self.finished == Some(TransferEnd::Complete)
            && self
                .files
                .iter()
                .all(|f| matches!(f.state, FileState::Received { .. }))
    }
}

/// One transfer in flight, either direction. Watch its progress, cancel it,
/// or wait for it to end.
pub struct Transfer {
    progress: watch::Sender<TransferProgress>,
    cancelled: AtomicBool,
    cancel: Notify,
}

impl std::fmt::Debug for Transfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Transfer({})", self.progress.borrow().transfer)
    }
}

impl Transfer {
    fn new(
        transfer: String,
        peer: PublicKey,
        outgoing: bool,
        files: &[FileDescriptor],
    ) -> Arc<Self> {
        let (progress, _) = watch::channel(TransferProgress {
            transfer,
            peer,
            outgoing,
            files: files.iter().map(pending).collect(),
            finished: None,
        });
        Arc::new(Self {
            progress,
            cancelled: AtomicBool::new(false),
            cancel: Notify::new(),
        })
    }

    /// The transfer id.
    #[must_use]
    pub fn id(&self) -> String {
        self.progress.borrow().transfer.clone()
    }

    /// A snapshot of the progress.
    #[must_use]
    pub fn progress(&self) -> TransferProgress {
        self.progress.borrow().clone()
    }

    /// A receiver that sees every change.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<TransferProgress> {
        self.progress.subscribe()
    }

    /// Cancels the transfer from this end: streams are reset and partial
    /// files removed.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.cancel.notify_waiters();
    }

    /// Whether a cancel was asked for, from either end.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Waits until the transfer is over and returns its final state.
    pub async fn wait(&self) -> TransferProgress {
        let mut rx = self.watch();
        loop {
            let snapshot = rx.borrow().clone();
            if snapshot.finished.is_some() {
                return snapshot;
            }
            if rx.changed().await.is_err() {
                return self.progress();
            }
        }
    }

    /// Resolves when a cancel is asked for; checks first so a cancel that
    /// came earlier is not missed.
    async fn cancelled_notified(&self) {
        loop {
            let notified = self.cancel.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }

    fn update(&self, f: impl FnOnce(&mut TransferProgress)) {
        self.progress.send_modify(f);
    }

    fn file(&self, id: &str, f: impl FnOnce(&mut FileProgress)) {
        self.update(|p| {
            if let Some(file) = p.files.iter_mut().find(|x| x.id == id) {
                f(file);
            }
        });
    }

    fn finish(&self, end: TransferEnd) {
        self.update(|p| {
            if p.finished.is_some() {
                return;
            }
            for file in &mut p.files {
                if matches!(file.state, FileState::Pending | FileState::Active) {
                    file.state = match &end {
                        TransferEnd::Cancelled => FileState::Cancelled,
                        TransferEnd::Declined { reason } => FileState::Failed {
                            reason: Some(*reason),
                            message: reason.to_string(),
                        },
                        TransferEnd::Failed { message } => FileState::Failed {
                            reason: None,
                            message: message.clone(),
                        },
                        TransferEnd::Complete => FileState::Failed {
                            reason: None,
                            message: "not answered".to_owned(),
                        },
                    };
                }
            }
            p.finished = Some(end);
        });
    }
}

fn pending(file: &FileDescriptor) -> FileProgress {
    FileProgress {
        id: file.id.clone(),
        name: file.name.clone(),
        size: file.size,
        done: 0,
        state: FileState::Pending,
    }
}

/// Something this device offers by id.
struct Offered {
    file: FileDescriptor,
    source: FileSource,
    /// The push this offer belongs to, if it is not standing.
    transfer: Option<String>,
}

/// The receiving side's view of one accepted transfer.
struct Incoming {
    peer: PublicKey,
    transfer: Arc<Transfer>,
    files: HashMap<String, FileDescriptor>,
    destination: Destination,
    pending: Mutex<HashSet<String>>,
    active: AtomicU32,
    changed: Notify,
}

impl Incoming {
    fn take(&self, id: &str) -> Option<FileDescriptor> {
        let mut pending = lock(&self.pending);
        if !pending.remove(id) {
            return None;
        }
        self.active.fetch_add(1, Ordering::SeqCst);
        self.files.get(id).cloned()
    }

    fn done(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.changed.notify_waiters();
    }

    fn drop_pending(&self) -> Vec<String> {
        let ids: Vec<String> = lock(&self.pending).drain().collect();
        self.changed.notify_waiters();
        ids
    }

    fn all_done(&self) -> bool {
        lock(&self.pending).is_empty() && self.active.load(Ordering::SeqCst) == 0
    }

    fn drained(&self) -> bool {
        self.active.load(Ordering::SeqCst) == 0
    }

    async fn wait_until(&self, done: impl Fn(&Self) -> bool) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if done(self) {
                return;
            }
            notified.await;
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The file synchroniser for one daemon.
pub struct FilesSync {
    daemon: Arc<Daemon>,
    store: Arc<dyn FileStore>,
    config: FilesConfig,
    offers: Mutex<HashMap<String, Offered>>,
    incoming: Mutex<HashMap<String, Arc<Incoming>>>,
    transfers: Mutex<HashMap<String, Arc<Transfer>>>,
    /// Per peer: the limit the semaphore was sized for, and the semaphore.
    budgets: Mutex<HashMap<PublicKey, (u32, Arc<Semaphore>)>>,
    receiving: AtomicU32,
    peak_receiving: AtomicU32,
    offered: tokio::sync::Mutex<Option<FilesParams>>,
    /// Every transfer as it starts, either direction, for hosts that show
    /// progress.
    started: tokio::sync::broadcast::Sender<Arc<Transfer>>,
}

impl std::fmt::Debug for FilesSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FilesSync")
    }
}

impl FilesSync {
    /// Starts carrying files for `daemon`, landing pushed ones in `store`:
    /// registers the stream handler and advertises the capability.
    #[must_use]
    pub fn start(daemon: Arc<Daemon>, store: Arc<dyn FileStore>, config: FilesConfig) -> Arc<Self> {
        let this = Arc::new(Self {
            daemon: daemon.clone(),
            store,
            config,
            offers: Mutex::new(HashMap::new()),
            incoming: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            budgets: Mutex::new(HashMap::new()),
            receiving: AtomicU32::new(0),
            peak_receiving: AtomicU32::new(0),
            offered: tokio::sync::Mutex::new(None),
            started: tokio::sync::broadcast::channel(32).0,
        });
        daemon.manager().on_stream(FILES_TYPE, this.handler());
        let advertise = this.clone();
        tokio::spawn(async move {
            loop {
                advertise.advertise().await;
                tokio::time::sleep(ADVERTISE_POLL).await;
            }
        });
        this
    }

    /// What this device advertises right now.
    #[must_use]
    pub fn params(&self) -> FilesParams {
        FilesParams {
            send: true,
            receive: self.store.available(),
            max_chunk_bytes: self
                .config
                .max_chunk_bytes
                .clamp(1, FilesParams::MAX_CHUNK_BYTES),
            max_streams: self.config.max_streams.max(1),
        }
    }

    /// Where pushed files land, for display.
    #[must_use]
    pub fn store_description(&self) -> String {
        self.store.describe()
    }

    /// Re-reads the store's availability and tells peers if the descriptor
    /// changed.
    pub async fn advertise(&self) {
        let mut offered = self.offered.lock().await;
        let params = self.params();
        if *offered == Some(params) {
            return;
        }
        self.daemon.registry().withdraw(FILES_TYPE).await;
        self.daemon.registry().offer(files_descriptor(params)).await;
        *offered = Some(params);
        tracing::info!(
            receive = params.receive,
            max_streams = params.max_streams,
            "files capability advertised"
        );
    }

    /// A fresh id: 128 random bits in base64url.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Randomness`] if the system's source fails.
    pub fn new_id() -> Result<String, Error> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| Error::Randomness(e.to_string()))?;
        Ok(Bytes(bytes.to_vec()).to_base64url())
    }

    /// Offers `file` by its id until withdrawn or the daemon stops.
    pub fn offer(&self, file: FileDescriptor, source: FileSource) {
        tracing::info!(id = %file.id, name = %file.name, size = file.size, "file offered");
        lock(&self.offers).insert(
            file.id.clone(),
            Offered {
                file,
                source,
                transfer: None,
            },
        );
    }

    /// Offers the file at `path` under a fresh id and returns its descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the path is not a readable regular file.
    pub fn offer_path(&self, path: &std::path::Path) -> Result<FileDescriptor, Error> {
        let (file, source) = FileSource::describe_path(&Self::new_id()?, path)?;
        self.offer(file.clone(), source);
        Ok(file)
    }

    /// Withdraws a standing offer. Returns whether there was one.
    pub fn withdraw(&self, id: &str) -> bool {
        let mut offers = lock(&self.offers);
        match offers.get(id) {
            Some(o) if o.transfer.is_none() => {
                offers.remove(id);
                tracing::info!(id, "offer withdrawn");
                true
            }
            _ => false,
        }
    }

    /// The standing offers.
    #[must_use]
    pub fn offers(&self) -> Vec<FileDescriptor> {
        let mut out: Vec<FileDescriptor> = lock(&self.offers)
            .values()
            .filter(|o| o.transfer.is_none())
            .map(|o| o.file.clone())
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        out
    }

    /// The transfers in flight.
    #[must_use]
    pub fn transfers(&self) -> Vec<TransferProgress> {
        let mut out: Vec<TransferProgress> = lock(&self.transfers)
            .values()
            .map(|t| t.progress())
            .collect();
        out.sort_by(|a, b| a.transfer.cmp(&b.transfer));
        out
    }

    /// A receiver of every transfer as it starts, either direction.
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Arc<Transfer>> {
        self.started.subscribe()
    }

    /// Records a transfer that just started and tells subscribers.
    fn track(&self, transfer: &Arc<Transfer>) {
        lock(&self.transfers).insert(transfer.id(), transfer.clone());
        let _ = self.started.send(transfer.clone());
    }

    /// A transfer in flight, by id.
    #[must_use]
    pub fn transfer(&self, id: &str) -> Option<Arc<Transfer>> {
        lock(&self.transfers).get(id).cloned()
    }

    /// Cancels a transfer in flight. Returns whether there was one.
    pub fn cancel(&self, id: &str) -> bool {
        match self.transfer(id) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    /// Data streams being received right now, over every peer.
    #[must_use]
    pub fn open_data_streams(&self) -> u32 {
        self.receiving.load(Ordering::SeqCst)
    }

    /// The most data streams received at once since start.
    #[must_use]
    pub fn peak_data_streams(&self) -> u32 {
        self.peak_receiving.load(Ordering::SeqCst)
    }

    /// A peer disconnected: its budget goes. Its transfers end on their own
    /// as their streams fail.
    pub fn detach(&self, peer: &PublicKey) {
        lock(&self.budgets).remove(peer);
    }

    /// Pushes `files` to `peer`, a binding of the same person that
    /// advertises `receive`. Returns at once with the transfer to watch.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if the peer is not connected,
    /// [`Error::Chain`] if it is not this person's, and [`Error::Decode`] if
    /// it does not receive files or the list is empty.
    pub fn push(
        self: &Arc<Self>,
        peer: &PublicKey,
        files: Vec<(FileDescriptor, FileSource)>,
    ) -> Result<Arc<Transfer>, Error> {
        if files.is_empty() {
            return Err(Error::Decode("nothing to send".to_owned()));
        }
        let (conn, params) = self.peer_for(peer)?;
        if !params.receive {
            return Err(Error::Decode(format!("{peer} does not receive files")));
        }
        let id = Self::new_id()?;
        let descriptors: Vec<FileDescriptor> = files.iter().map(|(f, _)| f.clone()).collect();
        let transfer = Transfer::new(id.clone(), *peer, true, &descriptors);
        self.track(&transfer);
        {
            let mut offers = lock(&self.offers);
            for (file, source) in &files {
                offers.insert(
                    file.id.clone(),
                    Offered {
                        file: file.clone(),
                        source: source.clone(),
                        transfer: Some(id.clone()),
                    },
                );
            }
        }
        let sync = self.clone();
        let t = transfer.clone();
        tokio::spawn(async move {
            sync.run_push(t, conn, params, files).await;
        });
        Ok(transfer)
    }

    /// Fetches `ids`, which `peer` offered earlier, into `destination`.
    /// Returns at once with the transfer to watch.
    ///
    /// # Errors
    ///
    /// As [`Self::push`], with [`Error::Decode`] if the peer does not send.
    pub fn fetch(
        self: &Arc<Self>,
        peer: &PublicKey,
        ids: Vec<String>,
        destination: Destination,
    ) -> Result<Arc<Transfer>, Error> {
        if ids.is_empty() {
            return Err(Error::Decode("nothing to fetch".to_owned()));
        }
        let (conn, params) = self.peer_for(peer)?;
        if !params.send {
            return Err(Error::Decode(format!("{peer} does not send files")));
        }
        let id = Self::new_id()?;
        let placeholders: Vec<FileDescriptor> =
            ids.iter().map(|i| FileDescriptor::new(i, i, 0)).collect();
        let transfer = Transfer::new(id.clone(), *peer, false, &placeholders);
        self.track(&transfer);
        let sync = self.clone();
        let t = transfer.clone();
        tokio::spawn(async move {
            sync.run_fetch(t, conn, ids, destination).await;
        });
        Ok(transfer)
    }

    /// The live connection to a same-person peer and its files params.
    fn peer_for(&self, peer: &PublicKey) -> Result<(Connection, FilesParams), Error> {
        let conn = self
            .daemon
            .manager()
            .peer(peer)
            .ok_or_else(|| Error::Transport(format!("not connected to {peer}")))?;
        if !self.same_person(peer) {
            return Err(Error::Chain(format!("{peer} is not a device of mine")));
        }
        let params = self
            .peer_params(peer)
            .ok_or_else(|| Error::Decode(format!("{peer} does not offer files")))?;
        Ok((conn, params))
    }

    fn peer_params(&self, peer: &PublicKey) -> Option<FilesParams> {
        self.daemon
            .registry()
            .peers()
            .into_iter()
            .find(|c| c.peer == *peer)
            .and_then(|c| c.capabilities.iter().find_map(FilesParams::from_descriptor))
    }

    fn same_person(&self, peer: &PublicKey) -> bool {
        matches!(
            self.daemon.recognise(peer),
            Ok(Recognition::MyBinding { .. })
        )
    }

    /// The semaphore bounding data streams to `peer`, sized to `limit`;
    /// resized when idle if the limit changed.
    fn budget(&self, peer: &PublicKey, limit: u32) -> Arc<Semaphore> {
        let mut budgets = lock(&self.budgets);
        let entry = budgets
            .entry(*peer)
            .or_insert_with(|| (limit, Arc::new(Semaphore::new(limit as usize))));
        if entry.0 != limit && entry.1.available_permits() == entry.0 as usize {
            *entry = (limit, Arc::new(Semaphore::new(limit as usize)));
        }
        entry.1.clone()
    }

    fn end_transfer(&self, transfer: &Transfer, end: TransferEnd) {
        let id = transfer.id();
        transfer.finish(end);
        lock(&self.transfers).remove(&id);
        lock(&self.incoming).remove(&id);
        lock(&self.offers).retain(|_, o| o.transfer.as_deref() != Some(id.as_str()));
        let p = transfer.progress();
        tracing::info!(
            transfer = %id,
            peer = %p.peer,
            outgoing = p.outgoing,
            bytes = p.done_bytes(),
            ?p.finished,
            "transfer ended"
        );
    }

    // --- the sender's side ---------------------------------------------------

    async fn run_push(
        self: Arc<Self>,
        transfer: Arc<Transfer>,
        conn: Connection,
        params: FilesParams,
        files: Vec<(FileDescriptor, FileSource)>,
    ) {
        let mut ctrl = match open_on(&conn, FILES_TYPE).await {
            Ok(s) => s,
            Err(e) => {
                self.end_transfer(
                    &transfer,
                    TransferEnd::Failed {
                        message: e.to_string(),
                    },
                );
                return;
            }
        };
        let offer = FilesMessage::Offer(FilesOffer {
            transfer: transfer.id(),
            files: files.iter().map(|(f, _)| f.clone()).collect(),
        });
        if let Err(e) = ctrl.send(&offer).await {
            self.end_transfer(
                &transfer,
                TransferEnd::Failed {
                    message: e.to_string(),
                },
            );
            return;
        }
        let accepted = match self.await_accept(&mut ctrl, &transfer).await {
            Ok(ids) => ids,
            Err(end) => {
                ctrl.reset(if end == TransferEnd::Cancelled {
                    CANCELLED_CODE
                } else {
                    FAILED_CODE
                });
                self.end_transfer(&transfer, end);
                return;
            }
        };
        let (accepted, refused): (Vec<_>, Vec<_>) = files
            .into_iter()
            .partition(|(f, _)| accepted.contains(&f.id));
        for (f, _) in refused {
            transfer.file(&f.id, |p| {
                p.state = FileState::Failed {
                    reason: Some(Reason::Refused),
                    message: "not accepted".to_owned(),
                };
            });
        }
        self.run_sender(transfer, conn, ctrl, params, accepted)
            .await;
    }

    /// Reads the receiver's answer to an offer, or a cancel from this end.
    async fn await_accept(
        &self,
        ctrl: &mut FramedStream,
        transfer: &Transfer,
    ) -> Result<Vec<String>, TransferEnd> {
        tokio::select! {
            answer = ctrl.recv::<FilesMessage>() => match answer {
                Ok(Some(FilesMessage::Accept(FilesAccept { ids }))) => Ok(ids),
                Ok(Some(FilesMessage::Decline(FilesDecline { reason }))) => {
                    Err(TransferEnd::Declined { reason })
                }
                Ok(Some(other)) => Err(TransferEnd::Failed {
                    message: format!("unexpected {other:?} instead of accept"),
                }),
                Ok(None) => Err(TransferEnd::Failed {
                    message: "control stream ended before accept".to_owned(),
                }),
                Err(e) => Err(TransferEnd::Failed { message: e.to_string() }),
            },
            () = transfer.cancelled_notified() => Err(TransferEnd::Cancelled),
        }
    }

    /// Sends the accepted files on data streams within the budget, watching
    /// the control stream for the receiver going away, then closes.
    async fn run_sender(
        self: Arc<Self>,
        transfer: Arc<Transfer>,
        conn: Connection,
        mut ctrl: FramedStream,
        params: FilesParams,
        accepted: Vec<(FileDescriptor, FileSource)>,
    ) {
        let limit = params.max_streams.min(self.config.send_streams).max(1);
        let semaphore = self.budget(conn.peer(), limit);
        let chunk = usize::try_from(
            params
                .max_chunk_bytes
                .clamp(1, FilesParams::MAX_CHUNK_BYTES),
        )
        .unwrap_or(FilesParams::DEFAULT_MAX_CHUNK_BYTES as usize);
        let mut set: JoinSet<()> = JoinSet::new();
        let sending = async {
            for (file, source) in accepted {
                let permit = tokio::select! {
                    p = semaphore.clone().acquire_owned() => p.ok(),
                    () = transfer.cancelled_notified() => None,
                };
                let Some(permit) = permit else { break };
                set.spawn(self.clone().send_one(
                    transfer.clone(),
                    conn.clone(),
                    file,
                    source,
                    chunk,
                    permit,
                ));
            }
            while set.join_next().await.is_some() {}
        };
        let mut peer_finished = false;
        tokio::select! {
            () = sending => {}
            answer = ctrl.recv::<FilesMessage>() => match answer {
                Ok(None) => {
                    // The receiver finished its side: every file it accepted
                    // has been answered, and our last tasks may still be
                    // reading those answers.
                    peer_finished = true;
                }
                other => {
                    // A reset, or a message where none is expected: the
                    // transfer is over from the receiver's end.
                    tracing::debug!(transfer = %transfer.id(), ?other, "receiver closed the control stream early");
                    transfer.cancel();
                }
            },
        }
        while set.join_next().await.is_some() {}
        if transfer.is_cancelled() {
            ctrl.reset(CANCELLED_CODE);
            self.end_transfer(&transfer, TransferEnd::Cancelled);
            return;
        }
        ctrl.finish();
        // The receiver finishes its side once every file is answered; a reset
        // instead means it cancelled while the last answers were in flight.
        let end = if peer_finished {
            TransferEnd::Complete
        } else {
            match tokio::time::timeout(FINISH_WAIT, ctrl.recv::<FilesMessage>()).await {
                Ok(Err(_)) => TransferEnd::Cancelled,
                _ => TransferEnd::Complete,
            }
        };
        self.end_transfer(&transfer, end);
    }

    /// Sends one file on its own data stream and reads the answer.
    async fn send_one(
        self: Arc<Self>,
        transfer: Arc<Transfer>,
        conn: Connection,
        file: FileDescriptor,
        source: FileSource,
        chunk: usize,
        _permit: OwnedSemaphorePermit,
    ) {
        let state = self
            .send_one_inner(&transfer, &conn, &file, &source, chunk)
            .await;
        transfer.file(&file.id, |p| p.state = state);
    }

    async fn send_one_inner(
        &self,
        transfer: &Transfer,
        conn: &Connection,
        file: &FileDescriptor,
        source: &FileSource,
        chunk: usize,
    ) -> FileState {
        let failed = |message: String| FileState::Failed {
            reason: None,
            message,
        };
        if transfer.is_cancelled() {
            return FileState::Cancelled;
        }
        transfer.file(&file.id, |p| p.state = FileState::Active);
        let mut stream = match open_on(conn, FILES_TYPE).await {
            Ok(s) => s,
            Err(e) => return failed(e.to_string()),
        };
        stream.set_priority(-1);
        let begin = FilesMessage::Begin(FilesBegin {
            transfer: transfer.id(),
            id: file.id.clone(),
        });
        if let Err(e) = stream.send(&begin).await {
            return failed(e.to_string());
        }
        let mut reader = match source.open().await {
            Ok(r) => r,
            Err(e) => {
                stream.reset(FAILED_CODE);
                return failed(e.to_string());
            }
        };
        let mut hasher = Sha256Stream::new();
        let mut buf = vec![0u8; chunk];
        let mut sent: u64 = 0;
        loop {
            if transfer.is_cancelled() {
                stream.reset(CANCELLED_CODE);
                return FileState::Cancelled;
            }
            let n = match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    stream.reset(FAILED_CODE);
                    return failed(format!("read: {e}"));
                }
            };
            hasher.update(&buf[..n]);
            let message = FilesMessage::Chunk(FilesChunk {
                bytes: Bytes(buf[..n].to_vec()),
            });
            if let Err(e) = stream.send(&message).await {
                return match peer_stop_code(&e) {
                    Some(CANCELLED_CODE) => FileState::Cancelled,
                    Some(code) => failed(format!("stopped by the receiver with code {code}")),
                    None if transfer.is_cancelled() => FileState::Cancelled,
                    None => failed(e.to_string()),
                };
            }
            sent += n as u64;
            transfer.file(&file.id, |p| p.done = sent);
        }
        if sent != file.size {
            stream.reset(FAILED_CODE);
            return FileState::Failed {
                reason: Some(Reason::SizeMismatch),
                message: format!("read {sent} bytes of a file declared as {}", file.size),
            };
        }
        let end = FilesMessage::End(FilesEnd {
            sha256: Bytes(hasher.finish().to_vec()).to_base64url(),
        });
        if let Err(e) = stream.send(&end).await {
            return failed(e.to_string());
        }
        stream.finish();
        match stream.recv::<FilesMessage>().await {
            Ok(Some(FilesMessage::Received)) => FileState::Received { location: None },
            Ok(Some(FilesMessage::Failed(FilesFailed { reason }))) => FileState::Failed {
                reason: Some(reason),
                message: reason.to_string(),
            },
            Ok(Some(other)) => failed(format!("unexpected {other:?} instead of received")),
            Ok(None) => failed("no answer from the receiver".to_owned()),
            Err(e) if transfer.is_cancelled() => {
                tracing::debug!(%e, "data stream ended after a cancel");
                FileState::Cancelled
            }
            Err(e) => failed(e.to_string()),
        }
    }

    // --- the receiver's side -------------------------------------------------

    async fn run_fetch(
        self: Arc<Self>,
        transfer: Arc<Transfer>,
        conn: Connection,
        ids: Vec<String>,
        destination: Destination,
    ) {
        let mut ctrl = match open_on(&conn, FILES_TYPE).await {
            Ok(s) => s,
            Err(e) => {
                self.end_transfer(
                    &transfer,
                    TransferEnd::Failed {
                        message: e.to_string(),
                    },
                );
                return;
            }
        };
        let request = FilesMessage::Request(FilesRequest {
            transfer: transfer.id(),
            ids: ids.clone(),
        });
        if let Err(e) = ctrl.send(&request).await {
            self.end_transfer(
                &transfer,
                TransferEnd::Failed {
                    message: e.to_string(),
                },
            );
            return;
        }
        let answer = tokio::select! {
            a = ctrl.recv::<FilesMessage>() => a,
            () = transfer.cancelled_notified() => {
                ctrl.reset(CANCELLED_CODE);
                self.end_transfer(&transfer, TransferEnd::Cancelled);
                return;
            }
        };
        let offer = match answer {
            Ok(Some(FilesMessage::Offer(o))) if o.transfer == transfer.id() => o,
            Ok(Some(FilesMessage::Decline(FilesDecline { reason }))) => {
                self.end_transfer(&transfer, TransferEnd::Declined { reason });
                return;
            }
            other => {
                ctrl.reset(FAILED_CODE);
                self.end_transfer(
                    &transfer,
                    TransferEnd::Failed {
                        message: format!("unexpected answer to the request: {other:?}"),
                    },
                );
                return;
            }
        };
        let offered: Vec<FileDescriptor> = offer
            .files
            .into_iter()
            .filter(|f| ids.contains(&f.id))
            .collect();
        transfer.update(|p| {
            for file in &mut p.files {
                match offered.iter().find(|f| f.id == file.id) {
                    Some(f) => {
                        file.name.clone_from(&f.name);
                        file.size = f.size;
                    }
                    None => {
                        file.state = FileState::Failed {
                            reason: Some(Reason::Gone),
                            message: "no longer offered".to_owned(),
                        };
                    }
                }
            }
        });
        if offered.is_empty() {
            let _ = ctrl
                .send(&FilesMessage::Decline(FilesDecline {
                    reason: Reason::Gone,
                }))
                .await;
            ctrl.finish();
            self.end_transfer(
                &transfer,
                TransferEnd::Declined {
                    reason: Reason::Gone,
                },
            );
            return;
        }
        let incoming = self.register_incoming(&transfer, conn.peer(), &offered, destination);
        let accept = FilesMessage::Accept(FilesAccept {
            ids: offered.iter().map(|f| f.id.clone()).collect(),
        });
        if let Err(e) = ctrl.send(&accept).await {
            self.end_transfer(
                &transfer,
                TransferEnd::Failed {
                    message: e.to_string(),
                },
            );
            return;
        }
        self.receive_until_done(ctrl, incoming).await;
    }

    fn register_incoming(
        &self,
        transfer: &Arc<Transfer>,
        peer: &PublicKey,
        files: &[FileDescriptor],
        destination: Destination,
    ) -> Arc<Incoming> {
        let incoming = Arc::new(Incoming {
            peer: *peer,
            transfer: transfer.clone(),
            files: files.iter().map(|f| (f.id.clone(), f.clone())).collect(),
            destination,
            pending: Mutex::new(files.iter().map(|f| f.id.clone()).collect()),
            active: AtomicU32::new(0),
            changed: Notify::new(),
        });
        lock(&self.incoming).insert(transfer.id(), incoming.clone());
        incoming
    }

    /// A peer pushes files to us: accept them into the store.
    async fn incoming_push(
        self: Arc<Self>,
        conn: Connection,
        mut ctrl: FramedStream,
        offer: FilesOffer,
    ) {
        let peer = *conn.peer();
        if !self.same_person(&peer) {
            tracing::warn!(%peer, "files offer from a device that is not mine");
            ctrl.reset(FAILED_CODE);
            return;
        }
        if !self.params().receive || offer.files.is_empty() {
            Self::decline(&mut ctrl, Reason::Refused).await;
            return;
        }
        let busy = {
            let incoming = lock(&self.incoming);
            incoming.contains_key(&offer.transfer)
                || incoming.values().filter(|i| i.peer == peer).count() >= MAX_PENDING_PER_PEER
        };
        if busy {
            Self::decline(&mut ctrl, Reason::Busy).await;
            return;
        }
        let transfer = Transfer::new(offer.transfer.clone(), peer, false, &offer.files);
        self.track(&transfer);
        let incoming =
            self.register_incoming(&transfer, &peer, &offer.files, Destination::Downloads);
        let accept = FilesMessage::Accept(FilesAccept {
            ids: offer.files.iter().map(|f| f.id.clone()).collect(),
        });
        if let Err(e) = ctrl.send(&accept).await {
            self.end_transfer(
                &transfer,
                TransferEnd::Failed {
                    message: e.to_string(),
                },
            );
            return;
        }
        tracing::info!(%peer, transfer = %transfer.id(), files = offer.files.len(), "files offered to us; accepted");
        self.receive_until_done(ctrl, incoming).await;
    }

    async fn decline(ctrl: &mut FramedStream, reason: Reason) {
        let _ = ctrl
            .send(&FilesMessage::Decline(FilesDecline { reason }))
            .await;
        ctrl.finish();
    }

    /// A peer asks for ids we offered: answer with what we still hold and
    /// send what it accepts.
    async fn incoming_fetch(
        self: Arc<Self>,
        conn: Connection,
        mut ctrl: FramedStream,
        request: FilesRequest,
    ) {
        let peer = *conn.peer();
        if !self.same_person(&peer) {
            tracing::warn!(%peer, "files request from a device that is not mine");
            ctrl.reset(FAILED_CODE);
            return;
        }
        let Some(params) = self.peer_params(&peer) else {
            Self::decline(&mut ctrl, Reason::Refused).await;
            return;
        };
        let found: Vec<(FileDescriptor, FileSource)> = {
            let offers = lock(&self.offers);
            request
                .ids
                .iter()
                .filter_map(|id| offers.get(id))
                .map(|o| (o.file.clone(), o.source.clone()))
                .collect()
        };
        if found.is_empty() {
            tracing::info!(%peer, ids = ?request.ids, "fetch of ids no longer offered");
            Self::decline(&mut ctrl, Reason::Gone).await;
            return;
        }
        let descriptors: Vec<FileDescriptor> = found.iter().map(|(f, _)| f.clone()).collect();
        let transfer = Transfer::new(request.transfer, peer, true, &descriptors);
        self.track(&transfer);
        let offer = FilesMessage::Offer(FilesOffer {
            transfer: transfer.id(),
            files: descriptors,
        });
        if let Err(e) = ctrl.send(&offer).await {
            self.end_transfer(
                &transfer,
                TransferEnd::Failed {
                    message: e.to_string(),
                },
            );
            return;
        }
        let accepted = match self.await_accept(&mut ctrl, &transfer).await {
            Ok(ids) => ids,
            Err(end) => {
                ctrl.reset(if end == TransferEnd::Cancelled {
                    CANCELLED_CODE
                } else {
                    FAILED_CODE
                });
                self.end_transfer(&transfer, end);
                return;
            }
        };
        let accepted: Vec<(FileDescriptor, FileSource)> = found
            .into_iter()
            .filter(|(f, _)| accepted.contains(&f.id))
            .collect();
        crate::grants::record_implicit_for(&self.daemon, &peer, FILES_TYPE);
        self.run_sender(transfer, conn, ctrl, params, accepted)
            .await;
    }

    /// Waits for every accepted file to be answered on its data stream, or
    /// for a cancel from either end, then closes the control stream.
    async fn receive_until_done(self: Arc<Self>, mut ctrl: FramedStream, incoming: Arc<Incoming>) {
        let transfer = incoming.transfer.clone();
        let mut peer_closed = false;
        let mut peer_finished = false;
        tokio::select! {
            () = incoming.wait_until(Incoming::all_done) => {}
            answer = ctrl.recv::<FilesMessage>() => match answer {
                Ok(None) => {
                    // The sender finished its side: it has sent every accepted
                    // file, and the last answers may still be in flight. A file
                    // it never began will not come now.
                    peer_finished = true;
                    for id in incoming.drop_pending() {
                        transfer.file(&id, |p| {
                            p.state = FileState::Failed {
                                reason: None,
                                message: "never sent".to_owned(),
                            };
                        });
                    }
                    incoming.wait_until(Incoming::drained).await;
                }
                other => {
                    // A reset, or a message where none is expected: the
                    // transfer is over from the sender's end.
                    tracing::debug!(transfer = %transfer.id(), ?other, "sender closed the control stream early");
                    peer_closed = true;
                    transfer.cancel();
                }
            },
            () = transfer.cancelled_notified() => {}
        }
        if transfer.is_cancelled() {
            // Files not started will never come; files in flight see the
            // flag and reset their streams.
            for id in incoming.drop_pending() {
                transfer.file(&id, |p| p.state = FileState::Cancelled);
            }
            incoming.wait_until(Incoming::drained).await;
            if !peer_closed {
                ctrl.reset(CANCELLED_CODE);
            }
            self.end_transfer(&transfer, TransferEnd::Cancelled);
            return;
        }
        ctrl.finish();
        if !peer_finished {
            let _ = tokio::time::timeout(FINISH_WAIT, ctrl.recv::<FilesMessage>()).await;
        }
        crate::grants::record_implicit_for(&self.daemon, &incoming.peer, FILES_TYPE);
        self.end_transfer(&transfer, TransferEnd::Complete);
        if incoming.destination == Destination::Downloads {
            self.notify_received(&incoming.peer, &transfer.progress());
        }
    }

    /// Lands one file from a data stream.
    async fn receive_data(
        self: Arc<Self>,
        conn: Connection,
        mut stream: FramedStream,
        begin: FilesBegin,
    ) {
        let peer = *conn.peer();
        let incoming = lock(&self.incoming).get(&begin.transfer).cloned();
        let Some(incoming) = incoming.filter(|i| i.peer == peer) else {
            tracing::debug!(%peer, transfer = %begin.transfer, "data stream for a transfer not accepted");
            stream.reset(FAILED_CODE);
            return;
        };
        let Some(file) = incoming.take(&begin.id) else {
            tracing::debug!(%peer, id = %begin.id, "data stream for a file not pending");
            stream.reset(FAILED_CODE);
            return;
        };
        let now = self.receiving.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak_receiving.fetch_max(now, Ordering::SeqCst);
        let transfer = incoming.transfer.clone();
        transfer.file(&file.id, |p| p.state = FileState::Active);
        let landing = match &incoming.destination {
            Destination::Downloads => self.store.open(&file),
            Destination::Path(path) => DirectoryStore::open_at(path, &file),
            Destination::Memory => Ok(MemoryLanding::open(&file)),
        };
        let state = match landing {
            Ok(landing) => Self::pump(&mut stream, landing, &file, &transfer).await,
            Err(e) => {
                tracing::warn!(%e, name = %file.name, "cannot start landing a file");
                Self::fail_file(&mut stream, Reason::WriteFailed).await;
                FileState::Failed {
                    reason: Some(Reason::WriteFailed),
                    message: e.to_string(),
                }
            }
        };
        transfer.file(&file.id, |p| p.state = state);
        self.receiving.fetch_sub(1, Ordering::SeqCst);
        incoming.done();
    }

    /// Tells the sender the file was not kept and stops reading.
    async fn fail_file(stream: &mut FramedStream, reason: Reason) {
        let _ = stream
            .send(&FilesMessage::Failed(FilesFailed { reason }))
            .await;
        stream.finish();
        stream.stop(FAILED_CODE);
    }

    /// Moves the chunks of one data stream into `landing` through a blocking
    /// writer, checks size and hash, and answers the sender.
    async fn pump(
        stream: &mut FramedStream,
        landing: Box<dyn Landing>,
        file: &FileDescriptor,
        transfer: &Transfer,
    ) -> FileState {
        let (tx, rx) = mpsc::channel::<WriterCommand>(4);
        let modified = file.modified;
        let writer = tokio::task::spawn_blocking(move || write_loop(landing, rx, modified));
        let mut written: u64 = 0;
        let failed = |reason: Reason| FileState::Failed {
            reason: Some(reason),
            message: reason.to_string(),
        };
        let outcome: Result<String, FileState> = loop {
            if transfer.is_cancelled() {
                stream.reset(CANCELLED_CODE);
                break Err(FileState::Cancelled);
            }
            match stream.recv::<FilesMessage>().await {
                Ok(Some(FilesMessage::Chunk(FilesChunk { bytes }))) => {
                    let len = bytes.0.len() as u64;
                    if written + len > file.size {
                        Self::fail_file(stream, Reason::SizeMismatch).await;
                        break Err(failed(Reason::SizeMismatch));
                    }
                    if tx.send(WriterCommand::Write(bytes.0)).await.is_err() {
                        // The writer stopped: it reports why below.
                        break Err(FileState::Failed {
                            reason: Some(Reason::WriteFailed),
                            message: "writer stopped".to_owned(),
                        });
                    }
                    written += len;
                    transfer.file(&file.id, |p| p.done = written);
                }
                Ok(Some(FilesMessage::End(FilesEnd { sha256 }))) => {
                    if written != file.size {
                        Self::fail_file(stream, Reason::SizeMismatch).await;
                        break Err(failed(Reason::SizeMismatch));
                    }
                    break Ok(sha256);
                }
                Ok(Some(other)) => {
                    tracing::debug!(?other, "unexpected message on a data stream");
                    stream.reset(FAILED_CODE);
                    break Err(FileState::Failed {
                        reason: None,
                        message: "unexpected message on the data stream".to_owned(),
                    });
                }
                Ok(None) => {
                    break Err(FileState::Failed {
                        reason: None,
                        message: "data stream ended before end".to_owned(),
                    });
                }
                Err(e) if transfer.is_cancelled() => {
                    tracing::debug!(%e, "data stream ended after a cancel");
                    break Err(FileState::Cancelled);
                }
                Err(e) => {
                    break Err(FileState::Failed {
                        reason: None,
                        message: e.to_string(),
                    });
                }
            }
        };
        match outcome {
            Ok(expected) => {
                let _ = tx.send(WriterCommand::Finish { expected }).await;
                drop(tx);
                match writer.await {
                    Ok(Ok(location)) => {
                        let _ = stream.send(&FilesMessage::Received).await;
                        stream.finish();
                        FileState::Received { location }
                    }
                    Ok(Err(reason)) => {
                        Self::fail_file(stream, reason).await;
                        failed(reason)
                    }
                    Err(e) => {
                        Self::fail_file(stream, Reason::WriteFailed).await;
                        FileState::Failed {
                            reason: Some(Reason::WriteFailed),
                            message: format!("writer task: {e}"),
                        }
                    }
                }
            }
            Err(state) => {
                let _ = tx.send(WriterCommand::Abort).await;
                drop(tx);
                let writer_reason = writer.await.ok().and_then(Result::err);
                match (state, writer_reason) {
                    // A write that failed is the better explanation.
                    (FileState::Failed { .. }, Some(reason)) => failed(reason),
                    (state, _) => state,
                }
            }
        }
    }

    /// A local notification that files landed, marked as this device's own
    /// copy so the notification monitor never forwards it.
    fn notify_received(&self, peer: &PublicKey, progress: &TransferProgress) {
        let received: Vec<&FileProgress> = progress
            .files
            .iter()
            .filter(|f| matches!(f.state, FileState::Received { .. }))
            .collect();
        if received.is_empty() {
            return;
        }
        let Some(backend) = self.daemon.notifications_backend() else {
            return;
        };
        let origin_name = self
            .daemon
            .device_display_name(peer)
            .unwrap_or_else(|| peer.fingerprint());
        let title = if received.len() == 1 {
            format!("Received {}", received[0].name)
        } else {
            format!("Received {} files", received.len())
        };
        let location = received.iter().find_map(|f| match &f.state {
            FileState::Received { location } => location.clone(),
            _ => None,
        });
        let notification = crate::notifications::Notification {
            id: format!("files-{}", progress.transfer),
            app_id: FILES_APP.to_owned(),
            app_name: "Ligature".to_owned(),
            title: Some(title),
            body: location,
            posted_at: Timestamp::now(),
            icon: None,
        };
        if let Err(e) = backend.show(peer, &origin_name, &notification) {
            tracing::debug!(%e, "cannot show the received-file notification");
        }
    }

    fn handler(self: &Arc<Self>) -> StreamHandler {
        let sync = self.clone();
        Arc::new(move |conn, mut stream| {
            let sync = sync.clone();
            Box::pin(async move {
                match stream.recv::<FilesMessage>().await {
                    Ok(Some(FilesMessage::Offer(offer))) => {
                        sync.incoming_push(conn, stream, offer).await;
                    }
                    Ok(Some(FilesMessage::Request(request))) => {
                        sync.incoming_fetch(conn, stream, request).await;
                    }
                    Ok(Some(FilesMessage::Begin(begin))) => {
                        sync.receive_data(conn, stream, begin).await;
                    }
                    other => {
                        tracing::debug!(peer = %conn.peer(), ?other, "files stream not opened with offer, request or begin");
                        stream.reset(FAILED_CODE);
                    }
                }
            })
        })
    }
}

/// What the async side tells the blocking writer.
enum WriterCommand {
    Write(Vec<u8>),
    Finish { expected: String },
    Abort,
}

/// Writes chunks as they come; on `Finish` compares the digest with the
/// sender's and puts the file in place. Returns where it landed, or why not.
fn write_loop(
    mut landing: Box<dyn Landing>,
    mut rx: mpsc::Receiver<WriterCommand>,
    modified: Option<Timestamp>,
) -> Result<Option<String>, Reason> {
    while let Some(command) = rx.blocking_recv() {
        match command {
            WriterCommand::Write(bytes) => {
                if let Err(e) = landing.write(&bytes) {
                    let reason = if e.to_string().contains("no space") {
                        Reason::NoSpace
                    } else {
                        Reason::WriteFailed
                    };
                    tracing::warn!(%e, "writing a received file failed");
                    landing.abort();
                    return Err(reason);
                }
            }
            WriterCommand::Finish { expected } => {
                if landing.digest() != expected {
                    landing.abort();
                    return Err(Reason::HashMismatch);
                }
                return match landing.finish(modified) {
                    Ok(landed) => {
                        tracing::info!(name = %landed.name, size = landed.size, ?landed.location, "file received");
                        Ok(landed.location)
                    }
                    Err(e) => {
                        tracing::warn!(%e, "putting a received file in place failed");
                        Err(Reason::WriteFailed)
                    }
                };
            }
            WriterCommand::Abort => break,
        }
    }
    landing.abort();
    Err(Reason::WriteFailed)
}
