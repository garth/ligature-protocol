//! The local clipboard behind one trait, with echo suppression. Spec chapter
//! 05 section 3b; milestone 1 tasks E2 to E4, milestone 5 LP-107.
//!
//! A clipboard holds one selection in several representations at once: the
//! plain text, other typed parts (HTML, an image) and a selection of files.
//! A platform implementation watches the system clipboard for changes,
//! reads what it can of a value, and sets a value that may carry promises:
//! parts whose bytes are fetched from the peer only when an application
//! asks for that type. The daemon never sees platform details. An in-memory
//! fake serves tests and the loopback end-to-end run.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use lp_core::Error;
use lp_core::capability::{ClipboardPart, ClipboardSet};
use lp_core::files::FileDescriptor;
use lp_core::wire::{Bytes, Timestamp};
use lp_crypto::hash::sha256;
use tokio::sync::broadcast;

/// The plain-text type, carried as `text` rather than a part.
pub const TEXT_PLAIN: &str = "text/plain";

/// Overrides where fetched clipboard content is cached (tests and runs).
pub const CACHE_DIR_ENV: &str = "LIGATURE_CLIPBOARD_CACHE";

/// One representation of a local selection, with its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// The content type.
    pub mime: String,
    /// The bytes.
    pub bytes: Arc<Vec<u8>>,
}

impl Part {
    /// A part of `mime` holding `bytes`.
    #[must_use]
    pub fn new(mime: &str, bytes: Vec<u8>) -> Self {
        Self {
            mime: mime.to_owned(),
            bytes: Arc::new(bytes),
        }
    }
}

/// A file in a local selection: on this device's disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    /// Where it is.
    pub path: PathBuf,
    /// Its base name.
    pub name: String,
    /// Its size.
    pub size: u64,
    /// Its content type.
    pub mime: String,
    /// When it was last modified.
    pub modified: Option<Timestamp>,
}

/// What the local clipboard holds, as read from the platform.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Value {
    /// The plain text, when the selection has one.
    pub text: Option<String>,
    /// The other representations, at most one per type.
    pub parts: Vec<Part>,
    /// A selection of files.
    pub files: Vec<LocalFile>,
}

impl Value {
    /// A value of plain text only.
    #[must_use]
    pub fn text(text: &str) -> Self {
        Self {
            text: Some(text.to_owned()),
            ..Self::default()
        }
    }

    /// Whether the value has nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.parts.is_empty() && self.files.is_empty()
    }

    /// The fingerprint of the value: each representation's type to the
    /// SHA-256 of its bytes, text under `text/plain`, and each file under
    /// its name.
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        let mut f = Fingerprint::default();
        if let Some(t) = &self.text {
            f.entries
                .insert(TEXT_PLAIN.to_owned(), sha256(t.as_bytes()));
        }
        for p in &self.parts {
            f.entries
                .insert(p.mime.to_ascii_lowercase(), sha256(&p.bytes));
        }
        for file in &self.files {
            f.entries.insert(
                format!("file:{}", file.name),
                sha256(&file.size.to_be_bytes()),
            );
        }
        f
    }
}

/// What identifies a value for the echo rule: the representation set's
/// hashes (spec chapter 05 section 3b).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fingerprint {
    /// Type, or `file:<name>`, to hash.
    pub entries: BTreeMap<String, [u8; 32]>,
}

impl Fingerprint {
    /// The fingerprint of a `set` as received: pushed parts by their bytes,
    /// announced parts by the hash the sender gave.
    #[must_use]
    pub fn of_set(set: &ClipboardSet) -> Self {
        let mut f = Self::default();
        if let Some(t) = &set.text {
            f.entries
                .insert(TEXT_PLAIN.to_owned(), sha256(t.as_bytes()));
        }
        for p in &set.parts {
            let hash = match (&p.bytes, &p.file) {
                (Some(b), _) => sha256(&b.0),
                (None, Some(file)) => announced_hash(file),
                (None, None) => continue,
            };
            f.entries.insert(p.mime.to_ascii_lowercase(), hash);
        }
        for file in &set.files {
            f.entries.insert(
                format!("file:{}", file.name),
                sha256(&file.size.to_be_bytes()),
            );
        }
        f
    }

    /// One hash over the whole set, for logs and counters.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut all = Vec::new();
        for (k, v) in &self.entries {
            all.extend_from_slice(k.as_bytes());
            all.push(0);
            all.extend_from_slice(v);
        }
        sha256(&all)
    }

    /// Whether every representation of `self` is in `other` with the same
    /// hash: what a platform that reads fewer types than were set reports
    /// after a set.
    #[must_use]
    pub fn is_within(&self, other: &Self) -> bool {
        !self.entries.is_empty()
            && self
                .entries
                .iter()
                .all(|(k, v)| other.entries.get(k) == Some(v))
    }
}

/// The 32 bytes behind a descriptor's `sha256`, or a hash of the id when it
/// cannot be decoded, so a bad announcement never matches anything local.
fn announced_hash(file: &FileDescriptor) -> [u8; 32] {
    file.sha256
        .as_deref()
        .and_then(|s| Bytes::from_base64url(s).ok())
        .and_then(|b| <[u8; 32]>::try_from(b.0).ok())
        .unwrap_or_else(|| sha256(file.id.as_bytes()))
}

/// A change seen on the local clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The new value, as much of it as the platform read.
    pub value: Value,
}

impl Change {
    /// A change to plain text.
    #[must_use]
    pub fn text(text: &str) -> Self {
        Self {
            value: Value::text(text),
        }
    }
}

/// An announced representation a peer will serve on request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promise {
    /// The content type.
    pub mime: String,
    /// The descriptor to fetch by.
    pub file: FileDescriptor,
}

/// What a fetch for a promise yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// The bytes, when the part was small enough to keep in memory.
    pub bytes: Option<Arc<Vec<u8>>>,
    /// The file in the clipboard cache otherwise, or for a file.
    pub path: Option<PathBuf>,
}

/// A future returned by a fetch.
pub type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<Fetched, Error>> + Send + 'a>>;

/// A future returned by a paste.
pub type PasteFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, Error>> + Send + 'a>>;

/// Fetches a promised id from the peer that announced it. The clipboard
/// implementation calls it when an application asks for that type; a second
/// call for the same id is answered from what the first fetched.
pub trait Fetch: Send + Sync {
    /// The bytes or cached file behind `id`.
    fn fetch(&self, id: &str) -> FetchFuture<'_>;
}

/// A value to put on the local clipboard: what a peer pushed, with the
/// promises for what it announced.
#[derive(Clone)]
pub struct Incoming {
    /// The plain text.
    pub text: Option<String>,
    /// Pushed parts, with their bytes.
    pub parts: Vec<Part>,
    /// Announced parts, fetched through `fetch` on request.
    pub promises: Vec<Promise>,
    /// A selection of files, fetched through `fetch` on request.
    pub files: Vec<FileDescriptor>,
    /// Serves the promises and the files.
    pub fetch: Option<Arc<dyn Fetch>>,
}

impl std::fmt::Debug for Incoming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Incoming")
            .field("text", &self.text.as_ref().map(String::len))
            .field(
                "parts",
                &self.parts.iter().map(|p| &p.mime).collect::<Vec<_>>(),
            )
            .field(
                "promises",
                &self.promises.iter().map(|p| &p.mime).collect::<Vec<_>>(),
            )
            .field("files", &self.files.len())
            .finish()
    }
}

impl Incoming {
    /// A value of text only.
    #[must_use]
    pub fn text(text: &str) -> Self {
        Self {
            text: Some(text.to_owned()),
            parts: Vec::new(),
            promises: Vec::new(),
            files: Vec::new(),
            fetch: None,
        }
    }

    /// The value with its promises and files dropped: what stays on the
    /// clipboard when the peer can no longer serve them.
    #[must_use]
    pub fn without_promises(&self) -> Self {
        Self {
            text: self.text.clone(),
            parts: self.parts.clone(),
            promises: Vec::new(),
            files: Vec::new(),
            fetch: None,
        }
    }

    /// Whether anything in it needs the peer.
    #[must_use]
    pub fn has_promises(&self) -> bool {
        !self.promises.is_empty() || !self.files.is_empty()
    }
}

/// The local clipboard. Implementations are cheap to call and may block
/// briefly; the daemon calls them from a blocking context where needed. A
/// platform that handles text only implements the text methods; the value
/// methods default to them.
pub trait Clipboard: Send + Sync {
    /// Whether the clipboard can currently be reached (a compositor is up).
    fn available(&self) -> bool;
    /// The current text, if the clipboard holds text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the clipboard cannot be read.
    fn read_text(&self) -> Result<Option<String>, Error>;
    /// Replaces the clipboard content with `text`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the clipboard cannot be written.
    fn set_text(&self, text: &str) -> Result<(), Error>;
    /// A receiver of every change, including ones this process made.
    fn subscribe(&self) -> broadcast::Receiver<Change>;
    /// The current value, as much of it as the platform can read.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the clipboard cannot be read.
    fn read_value(&self) -> Result<Option<Value>, Error> {
        Ok(self.read_text()?.map(|t| Value::text(&t)))
    }
    /// Replaces the clipboard content with `value`, holding its promises
    /// until the next change. The default sets the text and drops the rest.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the clipboard cannot be written.
    fn set_value(&self, value: &Incoming) -> Result<(), Error> {
        match &value.text {
            Some(t) => self.set_text(t),
            None => Err(Error::Io(
                "this clipboard takes text only and the value has none".to_owned(),
            )),
        }
    }
    /// The types this clipboard can read and set besides plain text: what
    /// the device advertises in `mimeTypes`. Text only by default.
    fn mime_types(&self) -> Vec<String> {
        vec![TEXT_PLAIN.to_owned()]
    }
    /// Whether this clipboard holds promises (`fetch` in the descriptor).
    fn holds_promises(&self) -> bool {
        false
    }
    /// What an application pasting `mime` would get, for diagnostics. The
    /// default answers from the pushed content; a clipboard that holds
    /// promises in the daemon's own process fetches them.
    fn paste(&self, mime: &str) -> PasteFuture<'_> {
        let mime = mime.to_owned();
        Box::pin(async move {
            let value = self
                .read_value()?
                .ok_or_else(|| Error::Io("the clipboard is empty".to_owned()))?;
            if mime.eq_ignore_ascii_case(TEXT_PLAIN) {
                return value
                    .text
                    .map(String::into_bytes)
                    .ok_or_else(|| Error::Io("no text on the clipboard".to_owned()));
            }
            value
                .parts
                .iter()
                .find(|p| p.mime.eq_ignore_ascii_case(&mime))
                .map(|p| p.bytes.as_ref().clone())
                .ok_or_else(|| Error::Io(format!("{mime} is not on the clipboard")))
        })
    }
}

/// Keeps a device from re-sending what it just received, and from applying
/// what it just sent: the fingerprint of the last value set locally by the
/// daemon and of the last value sent to peers.
#[derive(Debug, Default)]
pub struct EchoFilter {
    last_set: Mutex<Option<Fingerprint>>,
    last_sent: Mutex<Option<Fingerprint>>,
}

impl EchoFilter {
    /// The daemon is about to set the local clipboard to a value a peer
    /// sent; the change event that follows must not be sent back, and
    /// since both sides now hold it, copying it again later is not worth
    /// sending either.
    pub fn note_set(&self, value: &Fingerprint) {
        *self.lock(&self.last_set) = Some(value.clone());
        *self.lock(&self.last_sent) = Some(value.clone());
    }

    /// A local change happened: whether it is worth sending. False for the
    /// echo of our own set (what the platform read is within what was set)
    /// and for a repeat of what we last sent; true otherwise, in which case
    /// it is remembered as sent.
    pub fn should_send(&self, value: &Fingerprint) -> bool {
        if self
            .lock(&self.last_set)
            .take_if(|s| value.is_within(s))
            .is_some()
        {
            return false;
        }
        let mut sent = self.lock(&self.last_sent);
        if sent.as_ref().is_some_and(|s| value.is_within(s)) {
            return false;
        }
        *sent = Some(value.clone());
        true
    }

    /// A peer sent a value: whether applying it would change anything we
    /// know about. False if it equals what we last sent (they are echoing
    /// us).
    pub fn should_apply(&self, value: &Fingerprint) -> bool {
        self.lock(&self.last_sent).as_ref() != Some(value)
    }

    fn lock<'a>(
        &self,
        m: &'a Mutex<Option<Fingerprint>>,
    ) -> std::sync::MutexGuard<'a, Option<Fingerprint>> {
        m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Builds the parts of an outgoing value in the wire's shape, for tests and
/// the synchroniser: pushed parts carry their bytes.
#[must_use]
pub fn pushed_part(part: &Part) -> ClipboardPart {
    ClipboardPart::pushed(&part.mime, part.bytes.as_ref().clone())
}

/// What the fake holds: the value as set, with its promises, and what a
/// paste fetched.
#[derive(Default)]
struct FakeState {
    value: Option<Value>,
    incoming: Option<Incoming>,
}

/// What [`FakeClipboard::on_set`] runs.
type SetHook = Box<dyn Fn(&Incoming) + Send + Sync>;

/// An in-memory clipboard for tests and the loopback run: holds a value
/// with promises, and `paste` fetches one like an application would.
pub struct FakeClipboard {
    state: Mutex<FakeState>,
    changes: broadcast::Sender<Change>,
    available: Mutex<bool>,
    mime_types: Vec<String>,
    /// Runs inside `set_value`, as a platform reading the value at once
    /// would (a provider asked for a thumbnail, a clipboard manager).
    on_set: Mutex<Option<SetHook>>,
    /// Makes the next `set_value` fail, as a platform refusing a value.
    refuse_next: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for FakeClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FakeClipboard")
    }
}

impl Default for FakeClipboard {
    fn default() -> Self {
        let (changes, _) = broadcast::channel(16);
        Self {
            state: Mutex::new(FakeState::default()),
            changes,
            available: Mutex::new(true),
            mime_types: vec![
                TEXT_PLAIN.to_owned(),
                "text/html".to_owned(),
                "image/png".to_owned(),
                "text/uri-list".to_owned(),
            ],
            on_set: Mutex::new(None),
            refuse_next: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl FakeClipboard {
    /// A shared fake that takes text, HTML, PNG and file lists.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A shared fake that takes only the given types besides text.
    #[must_use]
    pub fn with_types(mime_types: &[&str]) -> Arc<Self> {
        let (changes, _) = broadcast::channel(16);
        Arc::new(Self {
            state: Mutex::new(FakeState::default()),
            changes,
            available: Mutex::new(true),
            mime_types: std::iter::once(TEXT_PLAIN)
                .chain(mime_types.iter().copied())
                .map(str::to_owned)
                .collect(),
            on_set: Mutex::new(None),
            refuse_next: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Runs `f` inside every later `set_value`, before the value is stored:
    /// what a platform that reads the value the moment it lands does, so a
    /// test can check what the daemon has recorded by then (LP-107).
    pub fn on_set(&self, f: impl Fn(&Incoming) + Send + Sync + 'static) {
        *self
            .on_set
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(f));
    }

    /// Makes the next `set_value` fail, as a platform that refuses the value.
    pub fn refuse_next_set(&self) {
        self.refuse_next
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// The user copied `text` in some application.
    pub fn user_copies(&self, text: &str) {
        self.user_copies_value(Value::text(text));
    }

    /// The user copied a whole value in some application.
    pub fn user_copies_value(&self, value: Value) {
        {
            let mut state = self.lock();
            state.value = Some(value.clone());
            state.incoming = None;
        }
        let _ = self.changes.send(Change { value });
    }

    /// Makes the clipboard reachable or not.
    pub fn set_available(&self, available: bool) {
        *self
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = available;
    }

    /// The types the clipboard currently offers, promises included.
    #[must_use]
    pub fn offered_types(&self) -> Vec<String> {
        let state = self.lock();
        let mut types = Vec::new();
        if let Some(v) = &state.value {
            if v.text.is_some() {
                types.push(TEXT_PLAIN.to_owned());
            }
            types.extend(v.parts.iter().map(|p| p.mime.clone()));
        }
        if let Some(i) = &state.incoming {
            types.extend(i.promises.iter().map(|p| p.mime.clone()));
            if !i.files.is_empty() {
                types.push("text/uri-list".to_owned());
            }
        }
        types
    }

    /// The promised file descriptors on the clipboard, if any.
    #[must_use]
    pub fn promised_files(&self) -> Vec<FileDescriptor> {
        self.lock()
            .incoming
            .as_ref()
            .map(|i| i.files.clone())
            .unwrap_or_default()
    }

    /// An application pastes `mime`: the bytes, fetched if promised.
    ///
    /// # Errors
    ///
    /// Returns the fetch's error, or [`Error::Io`] if the type is not on the
    /// clipboard.
    pub async fn paste(&self, mime: &str) -> Result<Vec<u8>, Error> {
        let (pushed, promise) = {
            let state = self.lock();
            let pushed = state.value.as_ref().and_then(|v| {
                if mime == TEXT_PLAIN {
                    v.text.as_ref().map(|t| t.as_bytes().to_vec())
                } else {
                    v.parts
                        .iter()
                        .find(|p| p.mime == mime)
                        .map(|p| p.bytes.as_ref().clone())
                }
            });
            let promise = state.incoming.as_ref().and_then(|i| {
                i.promises
                    .iter()
                    .find(|p| p.mime == mime)
                    .map(|p| (p.file.id.clone(), i.fetch.clone()))
            });
            (pushed, promise)
        };
        if let Some(bytes) = pushed {
            return Ok(bytes);
        }
        let Some((id, fetch)) = promise else {
            return Err(Error::Io(format!("{mime} is not on the clipboard")));
        };
        let fetch = fetch.ok_or_else(|| Error::Io("promise without a fetcher".to_owned()))?;
        let fetched = fetch.fetch(&id).await?;
        match (fetched.bytes, fetched.path) {
            (Some(b), _) => Ok(b.as_ref().clone()),
            (None, Some(p)) => {
                std::fs::read(&p).map_err(|e| Error::Io(format!("{}: {e}", p.display())))
            }
            (None, None) => Err(Error::Io("fetched nothing".to_owned())),
        }
    }

    /// A file manager pastes the file list: each file fetched to the cache,
    /// returned as the paths.
    ///
    /// # Errors
    ///
    /// Returns the fetch's error.
    pub async fn paste_files(&self) -> Result<Vec<PathBuf>, Error> {
        let (files, fetch) = {
            let state = self.lock();
            match &state.incoming {
                Some(i) => (i.files.clone(), i.fetch.clone()),
                None => (Vec::new(), None),
            }
        };
        let fetch = fetch.ok_or_else(|| Error::Io("no files on the clipboard".to_owned()))?;
        let mut paths = Vec::new();
        for f in files {
            let fetched = fetch.fetch(&f.id).await?;
            paths.push(
                fetched
                    .path
                    .ok_or_else(|| Error::Io("a file fetched to memory".to_owned()))?,
            );
        }
        Ok(paths)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Clipboard for FakeClipboard {
    fn available(&self) -> bool {
        *self
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn read_text(&self) -> Result<Option<String>, Error> {
        Ok(self.lock().value.as_ref().and_then(|v| v.text.clone()))
    }

    fn set_text(&self, text: &str) -> Result<(), Error> {
        self.set_value(&Incoming::text(text))
    }

    fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    fn read_value(&self) -> Result<Option<Value>, Error> {
        Ok(self.lock().value.clone())
    }

    fn set_value(&self, value: &Incoming) -> Result<(), Error> {
        if let Some(f) = self
            .on_set
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            f(value);
        }
        if self
            .refuse_next
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(Error::Io("the fake clipboard refused the value".to_owned()));
        }
        // What the platform reads back is the pushed content; promises
        // are served on paste and never read back as bytes.
        let read_back = Value {
            text: value.text.clone(),
            parts: value.parts.clone(),
            files: Vec::new(),
        };
        {
            let mut state = self.lock();
            state.value = Some(read_back.clone());
            state.incoming = Some(value.clone());
        }
        let _ = self.changes.send(Change { value: read_back });
        Ok(())
    }

    fn mime_types(&self) -> Vec<String> {
        self.mime_types.clone()
    }

    fn holds_promises(&self) -> bool {
        true
    }

    fn paste(&self, mime: &str) -> PasteFuture<'_> {
        let mime = mime.to_owned();
        Box::pin(async move { FakeClipboard::paste(self, &mime).await })
    }
}

/// The platform's clipboard, started: Wayland through the helper process on
/// Linux, `NSPasteboard` on macOS, nothing elsewhere.
#[must_use]
pub fn platform_clipboard() -> Option<Arc<dyn Clipboard>> {
    #[cfg(target_os = "linux")]
    {
        Some(crate::clipboard_linux::LinuxClipboard::start())
    }
    #[cfg(target_os = "macos")]
    {
        Some(crate::clipboard_macos::MacClipboard::start())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Where fetched clipboard content is cached before a paste completes
/// (plan decision 9): `clipboard/` under the daemon's runtime directory, so
/// two daemons on one machine never share one, unless [`CACHE_DIR_ENV`]
/// names another place.
#[must_use]
pub fn cache_dir(locations: &lp_core::paths::Locations) -> PathBuf {
    if let Some(dir) = std::env::var_os(CACHE_DIR_ENV).filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    locations.clipboard_cache()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_fake_reports_changes_and_reads_back() {
        let c = FakeClipboard::new();
        let mut rx = c.subscribe();
        c.user_copies("hello");
        assert_eq!(rx.recv().await.unwrap(), Change::text("hello"));
        assert_eq!(c.read_text().unwrap().as_deref(), Some("hello"));
        c.set_text("world").unwrap();
        assert_eq!(
            rx.recv().await.unwrap().value.text.as_deref(),
            Some("world")
        );
        assert!(c.available());
        c.set_available(false);
        assert!(!c.available());
    }

    #[test]
    fn the_echo_filter_suppresses_our_own_set_and_repeats() {
        let fp = |t: &str| Value::text(t).fingerprint();
        let f = EchoFilter::default();
        assert!(f.should_send(&fp("a")), "a fresh local copy is sent");
        assert!(
            !f.should_send(&fp("a")),
            "the same text again is not re-sent"
        );
        assert!(f.should_send(&fp("b")));
        // A peer sends "c": we set it locally; the change event must not go back.
        assert!(f.should_apply(&fp("c")));
        f.note_set(&fp("c"));
        assert!(!f.should_send(&fp("c")), "echo of our own set");
        assert!(
            !f.should_send(&fp("c")),
            "both sides hold it; copying it again is not sent"
        );
        // The peer echoing what both sides hold is not applied.
        assert!(!f.should_apply(&fp("c")));
        assert!(f.should_apply(&fp("d")));
        assert!(f.should_send(&fp("e")), "something new goes out");
        assert!(
            !f.should_apply(&fp("e")),
            "the peer echoing what we sent is not applied"
        );
    }

    #[test]
    fn a_platform_reading_fewer_types_is_still_an_echo() {
        let f = EchoFilter::default();
        let mut set = ClipboardSet::now("hi");
        set.parts = vec![ClipboardPart::pushed("text/html", b"<b>hi</b>".to_vec())];
        let full = Fingerprint::of_set(&set);
        f.note_set(&full);
        // The phone reports the text only after the set.
        assert!(!f.should_send(&Value::text("hi").fingerprint()));
        // A different text is a new value.
        assert!(f.should_send(&Value::text("ho").fingerprint()));
        // A local value with the same text but new html is new.
        f.note_set(&full);
        let mut local = Value::text("hi");
        local.parts = vec![Part::new("text/html", b"<i>hi</i>".to_vec())];
        assert!(f.should_send(&local.fingerprint()));
    }

    #[test]
    fn fingerprints_agree_between_a_value_and_its_set() {
        let png = vec![1u8, 2, 3];
        let mut local = Value::text("t");
        local.parts = vec![Part::new("image/png", png.clone())];
        let mut set = ClipboardSet::now("t");
        let mut file = FileDescriptor::new("p", "clipboard.png", 3);
        file.sha256 = Some(Bytes(sha256(&png).to_vec()).to_base64url());
        set.parts = vec![ClipboardPart::announced("image/png", file)];
        assert_eq!(local.fingerprint(), Fingerprint::of_set(&set));
    }
}
