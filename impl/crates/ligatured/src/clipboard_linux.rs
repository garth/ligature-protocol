//! The Linux clipboard over Wayland `wlr-data-control`/`ext-data-control`.
//! Milestone 1 decisions LP-29 and LP-34; milestone 5 LP-108.
//!
//! `wl-clipboard-rs` connects through `WAYLAND_DISPLAY`, and a daemon started
//! by systemd may not have that variable, or may start before the compositor.
//! Setting a process-wide environment variable at runtime is unsafe in Rust,
//! so the daemon runs the Wayland side in a small helper child process
//! (`ligatured clipboard-helper`) whose environment names the socket chosen
//! here: `WAYLAND_DISPLAY` when set, else the newest `wayland-N` socket in
//! `$XDG_RUNTIME_DIR`. Parent and child talk over the child's stdio in the
//! same length-prefixed CBOR frames as everything else, with a larger frame
//! bound since a clipboard value is not a wire message. When the compositor
//! goes away the helper exits and the supervisor retries every few seconds;
//! while no helper runs the clipboard is unavailable and not advertised.
//!
//! The helper watches the clipboard with `wl-clipboard-rs` and reads every
//! listed type of a value that another application set; it owns a
//! data-control source of its own for what the daemon sets, serving pushed
//! parts from its copy and asking the daemon (`fetch`) for an announced type
//! when the compositor requests it, so a paste of a promised image or file
//! list is what fetches the bytes.

use std::ffi::{OsStr, OsString};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lp_core::Error;
use lp_core::files::FileDescriptor;
use lp_core::frame;
use lp_core::wire::{Bytes, TypeName};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::clipboard::{Change, Clipboard, Fetch, Incoming, LocalFile, Part, TEXT_PLAIN, Value};

/// How often the supervisor looks for a compositor when there is none.
pub const RETRY: Duration = Duration::from_secs(3);

/// The largest frame between the daemon and its helper: a clipboard value
/// on a local pipe, not a wire message (ADR 0036 bounds those).
pub const HELPER_MAX_FRAME: usize = 256 * 1024 * 1024;

/// How long a paste of a promised type waits for the fetch before the
/// application gets nothing.
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// The id the helper asks for when a file manager pastes the file list.
const FILES_ID: &str = "files";

/// The type of a file list on the clipboard.
const URI_LIST: &str = "text/uri-list";

/// The types this clipboard reads and sets besides plain text.
const RICH_TYPES: [&str; 3] = ["text/html", "image/png", URI_LIST];

/// The plain-text types offered for a text, since applications ask for
/// any of them.
const TEXT_ALIASES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "TEXT",
    "STRING",
];

/// Type names of the helper's messages.
pub mod types {
    /// Helper to daemon: connected and watching.
    pub const READY: &str = "org.ligatureprotocol.clipboard-helper.ready";
    /// Helper to daemon: the clipboard now holds these parts.
    pub const CHANGED: &str = "org.ligatureprotocol.clipboard-helper.changed";
    /// Daemon to helper: offer these parts and promises on the clipboard.
    pub const SET: &str = "org.ligatureprotocol.clipboard-helper.set";
    /// Helper to daemon: an application asked for a promised type.
    pub const FETCH: &str = "org.ligatureprotocol.clipboard-helper.fetch";
    /// Daemon to helper: the bytes of a promised type, or none.
    pub const FETCHED: &str = "org.ligatureprotocol.clipboard-helper.fetched";
}

/// One representation with its bytes, as the helper reads or serves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperPart {
    /// The content type.
    pub mime: String,
    /// The bytes.
    pub bytes: Bytes,
}

/// A type the helper offers and asks the daemon for on request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperPromise {
    /// The content type.
    pub mime: String,
    /// What to ask for: a descriptor id, or `files` for the file list.
    pub id: String,
}

/// A message between the daemon and its clipboard helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperMessage {
    /// One of [`types`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// The parts, for `changed` and `set`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<HelperPart>,
    /// The promises, for `set`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub promises: Vec<HelperPromise>,
    /// The id, for `fetch` and `fetched`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The bytes, for `fetched`; none when the fetch failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Bytes>,
}

impl HelperMessage {
    fn new(t: &str) -> Self {
        Self {
            message_type: TypeName::new(t),
            parts: Vec::new(),
            promises: Vec::new(),
            id: None,
            bytes: None,
        }
    }
}

/// Picks the compositor socket (LP-34): `WAYLAND_DISPLAY` when set (a bare
/// name is relative to the runtime directory), else the newest `wayland-N`
/// entry in the runtime directory that is not a `.lock` file.
#[must_use]
pub fn select_wayland_socket(env_display: Option<&OsStr>, runtime_dir: &Path) -> Option<PathBuf> {
    if let Some(name) = env_display.filter(|n| !n.is_empty()) {
        let path = Path::new(name);
        return Some(if path.is_absolute() {
            path.to_path_buf()
        } else {
            runtime_dir.join(path)
        });
    }
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(runtime_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("wayland-")
                && !name.ends_with(".lock")
                && name["wayland-".len()..].chars().all(|c| c.is_ascii_digit())
        })
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, e.path()))
        })
        .collect();
    candidates.sort();
    candidates.pop().map(|(_, p)| p)
}

/// The daemon's view of one running helper.
struct Helper {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
}

/// What the daemon serves the helper's fetches from: the value's fetcher
/// and its file descriptors.
#[derive(Clone)]
struct Serving {
    fetch: Option<Arc<dyn Fetch>>,
    files: Vec<FileDescriptor>,
}

/// The Linux clipboard: a supervised helper process. Implements
/// [`Clipboard`] with a stable change channel across helper restarts.
pub struct LinuxClipboard {
    helper: Mutex<Option<Helper>>,
    last: Arc<Mutex<Option<Value>>>,
    serving: Arc<Mutex<Option<Serving>>>,
    changes: broadcast::Sender<Change>,
    runtime_dir: PathBuf,
    env_display: Option<OsString>,
    runtime: tokio::runtime::Handle,
}

impl std::fmt::Debug for LinuxClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LinuxClipboard(available={})", self.available())
    }
}

impl LinuxClipboard {
    /// Creates the clipboard for the current environment and starts its
    /// supervisor, which starts a helper whenever a compositor socket exists
    /// and no helper runs. Called within the daemon's runtime.
    #[must_use]
    pub fn start() -> Arc<Self> {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/run/user/self"));
        let env_display = std::env::var_os("WAYLAND_DISPLAY");
        Self::start_with(runtime_dir, env_display)
    }

    /// As [`Self::start`] with explicit environment, for tests.
    #[must_use]
    pub fn start_with(runtime_dir: PathBuf, env_display: Option<OsString>) -> Arc<Self> {
        let (changes, _) = broadcast::channel(32);
        let this = Arc::new(Self {
            helper: Mutex::new(None),
            last: Arc::new(Mutex::new(None)),
            serving: Arc::new(Mutex::new(None)),
            changes,
            runtime_dir,
            env_display,
            runtime: tokio::runtime::Handle::current(),
        });
        let supervised = this.clone();
        tokio::spawn(async move { supervised.supervise().await });
        this
    }

    async fn supervise(self: Arc<Self>) {
        loop {
            if !self.available() {
                match select_wayland_socket(self.env_display.as_deref(), &self.runtime_dir) {
                    Some(socket) if socket.exists() => match self.spawn_helper(&socket) {
                        Ok(()) => {
                            tracing::info!(socket = %socket.display(), "clipboard helper started")
                        }
                        Err(e) => tracing::debug!(%e, "clipboard helper did not start"),
                    },
                    _ => tracing::debug!("no Wayland socket; clipboard unavailable"),
                }
            }
            tokio::time::sleep(RETRY).await;
        }
    }

    fn spawn_helper(&self, socket: &Path) -> Result<(), Error> {
        let exe = std::env::current_exe().map_err(|e| Error::Io(format!("current exe: {e}")))?;
        let mut child = Command::new(exe)
            .arg("clipboard-helper")
            .env("WAYLAND_DISPLAY", socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| Error::Io(format!("spawn clipboard helper: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Io("helper stdin".to_owned()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Io("helper stdout".to_owned()))?;
        // Wait for ready so a compositor that refuses us is noticed now.
        let first = read_frame_sync(&mut stdout)?
            .ok_or_else(|| Error::Io("clipboard helper exited at once".to_owned()))?;
        let first: HelperMessage = frame::decode(&first)?;
        if first.message_type.as_str() != types::READY {
            return Err(Error::Io(format!(
                "helper said {} not ready",
                first.message_type
            )));
        }
        let stdin = Arc::new(Mutex::new(stdin));
        *self.lock_helper() = Some(Helper {
            child,
            stdin: stdin.clone(),
        });
        let changes = self.changes.clone();
        let last = Arc::clone(&self.last);
        let serving = Arc::clone(&self.serving);
        let runtime = self.runtime.clone();
        std::thread::spawn(move || {
            while let Ok(Some(body)) = read_frame_sync(&mut stdout) {
                let Ok(msg) = frame::decode::<HelperMessage>(&body) else {
                    continue;
                };
                match msg.message_type.as_str() {
                    types::CHANGED => {
                        let value = value_from_parts(msg.parts);
                        *lock(&last) = Some(value.clone());
                        let _ = changes.send(Change { value });
                    }
                    types::FETCH => {
                        let Some(id) = msg.id else { continue };
                        let serving = lock(&serving).clone();
                        let stdin = stdin.clone();
                        runtime.spawn(async move {
                            let bytes = serve_fetch(serving, &id).await;
                            let mut reply = HelperMessage::new(types::FETCHED);
                            reply.id = Some(id);
                            reply.bytes = bytes.map(Bytes);
                            if let Err(e) = write_to(&stdin, &reply) {
                                tracing::debug!(%e, "cannot answer the helper's fetch");
                            }
                        });
                    }
                    _ => {}
                }
            }
            tracing::info!("clipboard helper ended; clipboard unavailable until it restarts");
        });
        *lock(&self.last) = None;
        Ok(())
    }

    fn lock_helper(&self) -> std::sync::MutexGuard<'_, Option<Helper>> {
        lock(&self.helper)
    }

    fn send(&self, msg: &HelperMessage) -> Result<(), Error> {
        let slot = self.lock_helper();
        let helper = slot
            .as_ref()
            .ok_or_else(|| Error::Io("clipboard unavailable".to_owned()))?;
        write_to(&helper.stdin, msg)
    }
}

/// Serves one of the helper's fetches: the bytes of a promised part, or
/// the file list as `file:` URIs of the files fetched into the cache.
async fn serve_fetch(serving: Option<Serving>, id: &str) -> Option<Vec<u8>> {
    let serving = serving?;
    let fetch = serving.fetch.clone()?;
    if id == FILES_ID {
        let mut list = String::new();
        for f in &serving.files {
            let fetched = match fetch.fetch(&f.id).await {
                Ok(f) => f,
                Err(e) => {
                    tracing::debug!(%e, name = %f.name, "cannot fetch a clipboard file");
                    return None;
                }
            };
            let path = fetched.path?;
            list.push_str(&file_uri(&path));
            list.push_str("\r\n");
        }
        return Some(list.into_bytes());
    }
    match fetch.fetch(id).await {
        Ok(fetched) => match (fetched.bytes, fetched.path) {
            (Some(b), _) => Some(b.as_ref().clone()),
            (None, Some(p)) => std::fs::read(p).ok(),
            (None, None) => None,
        },
        Err(e) => {
            tracing::debug!(%e, id, "cannot fetch a clipboard part");
            None
        }
    }
}

/// The value behind what the helper read: the text, the files behind a
/// `text/uri-list` of local `file:` URIs, and the other parts as they are.
fn value_from_parts(parts: Vec<HelperPart>) -> Value {
    let mut value = Value::default();
    for part in parts {
        if part.mime.eq_ignore_ascii_case(TEXT_PLAIN) {
            value.text = Some(String::from_utf8_lossy(&part.bytes.0).into_owned());
        } else if part.mime.eq_ignore_ascii_case(URI_LIST) {
            let text = String::from_utf8_lossy(&part.bytes.0);
            let files: Vec<LocalFile> = text
                .lines()
                .filter_map(|l| local_path(l.trim()))
                .filter_map(|p| crate::clipboard_sync::local_file(&p).ok())
                .collect();
            if files.is_empty() {
                value.parts.push(Part {
                    mime: part.mime,
                    bytes: Arc::new(part.bytes.0),
                });
            } else {
                value.files = files;
            }
        } else {
            value.parts.push(Part {
                mime: part.mime,
                bytes: Arc::new(part.bytes.0),
            });
        }
    }
    value
}

/// The local path behind a `file:` URI, if it is one.
fn local_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path = match rest.find('/') {
        Some(0) => rest,
        Some(i) if rest[..i].eq_ignore_ascii_case("localhost") => &rest[i..],
        _ => return None,
    };
    Some(PathBuf::from(percent_decode(path)))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (hex(bytes.get(i + 1)), hex(bytes.get(i + 2)))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: Option<&u8>) -> Option<u8> {
    match b? {
        c @ b'0'..=b'9' => Some(c - b'0'),
        c @ b'a'..=b'f' => Some(c - b'a' + 10),
        c @ b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// A `file:` URI for a local path, with the unreserved characters and `/`
/// kept and everything else percent-encoded.
#[must_use]
pub fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

impl Clipboard for LinuxClipboard {
    fn available(&self) -> bool {
        let mut slot = self.lock_helper();
        let alive = match slot.as_mut() {
            Some(h) => matches!(h.child.try_wait(), Ok(None)),
            None => false,
        };
        if !alive {
            *slot = None;
        }
        alive
    }

    fn read_text(&self) -> Result<Option<String>, Error> {
        Ok(lock(&self.last).as_ref().and_then(|v| v.text.clone()))
    }

    fn set_text(&self, text: &str) -> Result<(), Error> {
        self.set_value(&Incoming::text(text))
    }

    fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    fn read_value(&self) -> Result<Option<Value>, Error> {
        Ok(lock(&self.last).clone())
    }

    fn set_value(&self, value: &Incoming) -> Result<(), Error> {
        let mut msg = HelperMessage::new(types::SET);
        if let Some(t) = &value.text {
            msg.parts.push(HelperPart {
                mime: TEXT_PLAIN.to_owned(),
                bytes: Bytes(t.clone().into_bytes()),
            });
        }
        let has_files = !value.files.is_empty();
        for p in &value.parts {
            // The sender's own file list names its paths; ours comes from
            // the fetched files.
            if has_files && p.mime.eq_ignore_ascii_case(URI_LIST) {
                continue;
            }
            msg.parts.push(HelperPart {
                mime: p.mime.clone(),
                bytes: Bytes(p.bytes.as_ref().clone()),
            });
        }
        for p in &value.promises {
            msg.promises.push(HelperPromise {
                mime: p.mime.clone(),
                id: p.file.id.clone(),
            });
        }
        if has_files {
            msg.promises.push(HelperPromise {
                mime: URI_LIST.to_owned(),
                id: FILES_ID.to_owned(),
            });
        }
        *lock(&self.serving) = Some(Serving {
            fetch: value.fetch.clone(),
            files: value.files.clone(),
        });
        self.send(&msg)?;
        *lock(&self.last) = Some(Value {
            text: value.text.clone(),
            parts: value.parts.clone(),
            files: Vec::new(),
        });
        Ok(())
    }

    fn mime_types(&self) -> Vec<String> {
        std::iter::once(TEXT_PLAIN)
            .chain(RICH_TYPES)
            .map(str::to_owned)
            .collect()
    }

    fn holds_promises(&self) -> bool {
        true
    }
}

fn write_to(stdin: &Mutex<ChildStdin>, msg: &HelperMessage) -> Result<(), Error> {
    let bytes = frame::encode(msg, HELPER_MAX_FRAME)?;
    let mut stdin = lock(stdin);
    stdin
        .write_all(&bytes)
        .and_then(|()| stdin.flush())
        .map_err(|e| Error::Io(format!("clipboard helper: {e}")))
}

fn read_frame_sync<R: std::io::Read>(r: &mut R) -> Result<Option<Vec<u8>>, Error> {
    let mut prefix = [0u8; 4];
    match r.read_exact(&mut prefix) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(Error::Io(format!("helper read: {e}"))),
    }
    let len = frame::body_len(prefix, HELPER_MAX_FRAME)?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)
        .map_err(|e| Error::Io(format!("helper read: {e}")))?;
    Ok(Some(body))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The text type to request from an offer, if it has one.
#[must_use]
pub fn pick_text_mime(offered: &[String]) -> Option<&str> {
    TEXT_ALIASES
        .iter()
        .find_map(|p| offered.iter().find(|m| m.eq_ignore_ascii_case(p)))
        .map(String::as_str)
}

/// The helper process: watches the Wayland clipboard, serves the daemon's
/// `set` requests through its own data-control source, and asks the daemon
/// for promised types when an application pastes them, until stdin closes.
/// Runs in the child; never returns normally while the compositor is up.
///
/// # Errors
///
/// Returns [`Error::Io`] if the compositor cannot be reached.
pub fn run_helper() -> Result<(), Error> {
    use wl_clipboard_rs::paste::Seat;
    use wl_clipboard_rs::watch::{ClipboardEvent, ClipboardType, Watcher};

    let mut watcher = Watcher::new(ClipboardType::Regular, Seat::Unspecified)
        .map_err(|e| Error::Io(format!("wayland clipboard: {e}")))?;
    let cancel = watcher.cancel_handle();
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let shared = Arc::new(helper::Shared::default());
    let source = helper::SourceLoop::start(stdout.clone(), shared.clone())?;
    write_sync(&stdout, &HelperMessage::new(types::READY))?;

    // stdin: set requests and fetch answers. EOF means the daemon went away.
    {
        let shared = shared.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            while let Ok(Some(body)) = read_frame_sync(&mut stdin) {
                let Ok(msg) = frame::decode::<HelperMessage>(&body) else {
                    continue;
                };
                match msg.message_type.as_str() {
                    types::SET => source.set(msg),
                    types::FETCHED => {
                        if let Some(id) = msg.id {
                            shared.answer(&id, msg.bytes.map(|b| b.0));
                        }
                    }
                    _ => {}
                }
            }
            cancel.cancel();
            std::process::exit(0);
        });
    }

    loop {
        match watcher.next_event() {
            Ok(Some(ClipboardEvent::Changed {
                mime_types,
                mut offer,
                ..
            })) => {
                if shared.is_ours(&mime_types) {
                    continue;
                }
                let mut parts = Vec::new();
                if let Some(mime) = pick_text_mime(&mime_types)
                    && let Ok(mut reader) = offer.receive(mime)
                {
                    let mut bytes = Vec::new();
                    if reader.read_to_end(&mut bytes).is_ok() {
                        parts.push(HelperPart {
                            mime: TEXT_PLAIN.to_owned(),
                            bytes: Bytes(bytes),
                        });
                    }
                }
                for wanted in RICH_TYPES {
                    let Some(mime) = mime_types.iter().find(|m| m.eq_ignore_ascii_case(wanted))
                    else {
                        continue;
                    };
                    let Ok(mut reader) = offer.receive(mime) else {
                        continue;
                    };
                    let mut bytes = Vec::new();
                    if reader.read_to_end(&mut bytes).is_ok() && !bytes.is_empty() {
                        parts.push(HelperPart {
                            mime: wanted.to_owned(),
                            bytes: Bytes(bytes),
                        });
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                let mut msg = HelperMessage::new(types::CHANGED);
                msg.parts = parts;
                write_sync(&stdout, &msg)?;
            }
            Ok(Some(ClipboardEvent::Cleared { .. })) => {}
            Ok(None) => return Ok(()),
            Err(e) => return Err(Error::Io(format!("wayland clipboard: {e}"))),
        }
    }
}

fn write_sync(out: &Arc<Mutex<std::io::Stdout>>, msg: &HelperMessage) -> Result<(), Error> {
    let bytes = frame::encode(msg, HELPER_MAX_FRAME)?;
    let mut out = lock(out);
    out.write_all(&bytes)
        .and_then(|()| out.flush())
        .map_err(|e| Error::Io(format!("helper write: {e}")))
}

/// The helper's own data-control source: what the daemon sets, served to
/// the compositor on request.
mod helper {
    use std::collections::{HashMap, HashSet};
    use std::fs::File;
    use std::io::{Read as _, Write as _};
    use std::os::fd::OwnedFd;
    use std::sync::{Arc, Mutex, mpsc};

    use lp_core::Error;
    use rustix::event::{PollFd, PollFlags};
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::protocol::wl_seat::WlSeat;
    use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle, event_created_child};
    use wayland_protocols::ext::data_control::v1::client as ext;
    use wayland_protocols_wlr::data_control::v1::client as zwlr;

    use super::{FETCH_TIMEOUT, HelperMessage, TEXT_ALIASES, lock, types, write_sync};
    use crate::clipboard::TEXT_PLAIN;

    /// Those waiting for one fetched id.
    type Waiters = Vec<mpsc::Sender<Option<Vec<u8>>>>;

    /// What the watcher and the source share: the types we currently own
    /// the selection with, and the fetches awaiting the daemon's answer.
    #[derive(Default)]
    pub struct Shared {
        owned: Mutex<Option<HashSet<String>>>,
        pending: Mutex<HashMap<String, Waiters>>,
    }

    impl Shared {
        /// Whether a change the watcher saw is our own selection.
        pub fn is_ours(&self, mime_types: &[String]) -> bool {
            let owned = lock(&self.owned);
            let Some(owned) = owned.as_ref() else {
                return false;
            };
            let seen: HashSet<String> = mime_types.iter().cloned().collect();
            seen == *owned
        }

        /// The daemon answered a fetch.
        pub fn answer(&self, id: &str, bytes: Option<Vec<u8>>) {
            if let Some(waiters) = lock(&self.pending).remove(id) {
                for w in waiters {
                    let _ = w.send(bytes.clone());
                }
            }
        }

        fn wait_for(&self, id: &str) -> mpsc::Receiver<Option<Vec<u8>>> {
            let (tx, rx) = mpsc::channel();
            lock(&self.pending)
                .entry(id.to_owned())
                .or_default()
                .push(tx);
            rx
        }
    }

    /// Either data-control protocol's objects.
    #[derive(Clone)]
    enum Manager {
        Zwlr(zwlr::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1),
        Ext(ext::ext_data_control_manager_v1::ExtDataControlManagerV1),
    }

    #[derive(Clone)]
    enum Device {
        Zwlr(zwlr::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1),
        Ext(ext::ext_data_control_device_v1::ExtDataControlDeviceV1),
    }

    #[derive(Clone, PartialEq)]
    enum Source {
        Zwlr(zwlr::zwlr_data_control_source_v1::ZwlrDataControlSourceV1),
        Ext(ext::ext_data_control_source_v1::ExtDataControlSourceV1),
    }

    impl Source {
        fn offer(&self, mime: String) {
            match self {
                Self::Zwlr(s) => s.offer(mime),
                Self::Ext(s) => s.offer(mime),
            }
        }
        fn destroy(&self) {
            match self {
                Self::Zwlr(s) => s.destroy(),
                Self::Ext(s) => s.destroy(),
            }
        }
    }

    impl Device {
        fn set_selection(&self, source: &Source) {
            match (self, source) {
                (Self::Zwlr(d), Source::Zwlr(s)) => d.set_selection(Some(s)),
                (Self::Ext(d), Source::Ext(s)) => d.set_selection(Some(s)),
                _ => {}
            }
        }
    }

    /// What a type is served from.
    #[derive(Clone)]
    enum Serve {
        Bytes(Arc<Vec<u8>>),
        Promise(String),
    }

    /// The selection we currently own, if any.
    struct Current {
        source: Source,
        serve: HashMap<String, Serve>,
    }

    struct State {
        manager: Manager,
        devices: Vec<Device>,
        current: Option<Current>,
        shared: Arc<Shared>,
        stdout: Arc<Mutex<std::io::Stdout>>,
    }

    impl State {
        fn apply(&mut self, qh: &QueueHandle<Self>, msg: HelperMessage) {
            if let Some(old) = self.current.take() {
                old.source.destroy();
            }
            let mut serve: HashMap<String, Serve> = HashMap::new();
            for p in msg.parts {
                let bytes = Arc::new(p.bytes.0);
                if p.mime.eq_ignore_ascii_case(TEXT_PLAIN) {
                    for alias in TEXT_ALIASES {
                        serve.insert(alias.to_owned(), Serve::Bytes(bytes.clone()));
                    }
                } else {
                    serve.insert(p.mime, Serve::Bytes(bytes));
                }
            }
            for p in msg.promises {
                serve.insert(p.mime, Serve::Promise(p.id));
            }
            if serve.is_empty() {
                *lock(&self.shared.owned) = None;
                return;
            }
            let source = match &self.manager {
                Manager::Zwlr(m) => Source::Zwlr(m.create_data_source(qh, ())),
                Manager::Ext(m) => Source::Ext(m.create_data_source(qh, ())),
            };
            for mime in serve.keys() {
                source.offer(mime.clone());
            }
            for d in &self.devices {
                d.set_selection(&source);
            }
            *lock(&self.shared.owned) = Some(serve.keys().cloned().collect());
            self.current = Some(Current { source, serve });
        }

        fn send(&mut self, source: &Source, mime: String, fd: OwnedFd) {
            let Some(current) = self.current.as_ref().filter(|c| c.source == *source) else {
                return;
            };
            let Some(what) = current.serve.get(&mime).cloned() else {
                return;
            };
            let shared = self.shared.clone();
            let stdout = self.stdout.clone();
            std::thread::spawn(move || {
                let bytes = match what {
                    Serve::Bytes(b) => Some(b),
                    Serve::Promise(id) => {
                        let rx = shared.wait_for(&id);
                        let mut ask = HelperMessage::new(types::FETCH);
                        ask.id = Some(id);
                        if write_sync(&stdout, &ask).is_err() {
                            return;
                        }
                        rx.recv_timeout(FETCH_TIMEOUT).ok().flatten().map(Arc::new)
                    }
                };
                let Some(bytes) = bytes else {
                    drop(fd);
                    return;
                };
                // Clear O_NONBLOCK, else a large write stops halfway.
                let _ = rustix::fs::fcntl_setfl(&fd, rustix::fs::OFlags::empty());
                // EPIPE means the application read as much as it wanted.
                let _ = File::from(fd).write_all(&bytes);
            });
        }

        fn cancelled(&mut self, source: &Source) {
            if self.current.as_ref().is_some_and(|c| c.source == *source) {
                self.current = None;
                *lock(&self.shared.owned) = None;
            }
            source.destroy();
        }
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: wayland_client::protocol::wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<WlSeat, ()> for State {
        fn event(
            _: &mut Self,
            _: &WlSeat,
            _: wayland_client::protocol::wl_seat::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    macro_rules! quiet {
        ($($iface:ty),*) => {
            $(
                impl Dispatch<$iface, ()> for State {
                    fn event(
                        _: &mut Self,
                        _: &$iface,
                        _: <$iface as wayland_client::Proxy>::Event,
                        _: &(),
                        _: &Connection,
                        _: &QueueHandle<Self>,
                    ) {
                    }
                }
            )*
        };
    }
    quiet!(
        zwlr::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
        ext::ext_data_control_manager_v1::ExtDataControlManagerV1,
        zwlr::zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
        ext::ext_data_control_offer_v1::ExtDataControlOfferV1
    );

    impl Dispatch<zwlr::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, ()> for State {
        fn event(
            _: &mut Self,
            _: &zwlr::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
            event: zwlr::zwlr_data_control_device_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let zwlr::zwlr_data_control_device_v1::Event::DataOffer { id } = event {
                id.destroy();
            }
        }
        event_created_child!(State, zwlr::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, [
            zwlr::zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (zwlr::zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, ()),
        ]);
    }

    impl Dispatch<ext::ext_data_control_device_v1::ExtDataControlDeviceV1, ()> for State {
        fn event(
            _: &mut Self,
            _: &ext::ext_data_control_device_v1::ExtDataControlDeviceV1,
            event: ext::ext_data_control_device_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let ext::ext_data_control_device_v1::Event::DataOffer { id } = event {
                id.destroy();
            }
        }
        event_created_child!(State, ext::ext_data_control_device_v1::ExtDataControlDeviceV1, [
            ext::ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ext::ext_data_control_offer_v1::ExtDataControlOfferV1, ()),
        ]);
    }

    impl Dispatch<zwlr::zwlr_data_control_source_v1::ZwlrDataControlSourceV1, ()> for State {
        fn event(
            state: &mut Self,
            proxy: &zwlr::zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
            event: zwlr::zwlr_data_control_source_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            use zwlr::zwlr_data_control_source_v1::Event;
            let source = Source::Zwlr(proxy.clone());
            match event {
                Event::Send { mime_type, fd } => state.send(&source, mime_type, fd),
                Event::Cancelled => state.cancelled(&source),
                _ => {}
            }
        }
    }

    impl Dispatch<ext::ext_data_control_source_v1::ExtDataControlSourceV1, ()> for State {
        fn event(
            state: &mut Self,
            proxy: &ext::ext_data_control_source_v1::ExtDataControlSourceV1,
            event: ext::ext_data_control_source_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            use ext::ext_data_control_source_v1::Event;
            let source = Source::Ext(proxy.clone());
            match event {
                Event::Send { mime_type, fd } => state.send(&source, mime_type, fd),
                Event::Cancelled => state.cancelled(&source),
                _ => {}
            }
        }
    }

    /// The thread that owns the source connection; `set` hands it a value.
    pub struct SourceLoop {
        sets: mpsc::Sender<HelperMessage>,
        wake: Mutex<File>,
    }

    impl SourceLoop {
        /// Connects to the compositor named by the environment and starts
        /// the loop.
        ///
        /// # Errors
        ///
        /// Returns [`Error::Io`] if the compositor or the data-control
        /// protocol is not there.
        pub fn start(
            stdout: Arc<Mutex<std::io::Stdout>>,
            shared: Arc<Shared>,
        ) -> Result<Arc<Self>, Error> {
            let conn = Connection::connect_to_env()
                .map_err(|e| Error::Io(format!("wayland source: {e}")))?;
            let (globals, mut queue) = registry_queue_init::<State>(&conn)
                .map_err(|e| Error::Io(format!("wayland registry: {e}")))?;
            let qh = queue.handle();
            let manager = if let Ok(m) = globals.bind(&qh, 1..=1, ()) {
                Manager::Ext(m)
            } else if let Ok(m) = globals.bind(&qh, 1..=1, ()) {
                Manager::Zwlr(m)
            } else {
                return Err(Error::Io(
                    "neither ext-data-control nor wlr-data-control".to_owned(),
                ));
            };
            let registry = globals.registry();
            let seats: Vec<WlSeat> = globals.contents().with_list(|list| {
                list.iter()
                    .filter(|g| g.interface == WlSeat::interface().name && g.version >= 2)
                    .map(|g| registry.bind(g.name, 2, &qh, ()))
                    .collect()
            });
            if seats.is_empty() {
                return Err(Error::Io("no Wayland seat".to_owned()));
            }
            let devices: Vec<Device> = seats
                .iter()
                .map(|seat| match &manager {
                    Manager::Zwlr(m) => Device::Zwlr(m.get_data_device(seat, &qh, ())),
                    Manager::Ext(m) => Device::Ext(m.get_data_device(seat, &qh, ())),
                })
                .collect();
            let mut state = State {
                manager,
                devices,
                current: None,
                shared,
                stdout,
            };
            queue
                .roundtrip(&mut state)
                .map_err(|e| Error::Io(format!("wayland roundtrip: {e}")))?;
            let (wake_r, wake_w) = rustix::pipe::pipe_with(
                rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
            )
            .map_err(|e| Error::Io(format!("pipe: {e}")))?;
            let (sets, set_rx) = mpsc::channel::<HelperMessage>();
            let this = Arc::new(Self {
                sets,
                wake: Mutex::new(File::from(wake_w)),
            });
            std::thread::spawn(move || {
                let mut wake_r = File::from(wake_r);
                if let Err(e) = run(&mut queue, &mut state, &set_rx, &mut wake_r) {
                    eprintln!("clipboard helper: source loop ended: {e}");
                    std::process::exit(1);
                }
            });
            Ok(this)
        }

        /// Puts the message's parts and promises on the clipboard.
        pub fn set(&self, msg: HelperMessage) {
            let _ = self.sets.send(msg);
            let _ = lock(&self.wake).write_all(&[1]);
        }
    }

    fn run(
        queue: &mut wayland_client::EventQueue<State>,
        state: &mut State,
        sets: &mpsc::Receiver<HelperMessage>,
        wake: &mut File,
    ) -> Result<(), Error> {
        let qh = queue.handle();
        loop {
            while let Ok(msg) = sets.try_recv() {
                state.apply(&qh, msg);
            }
            queue
                .flush()
                .map_err(|e| Error::Io(format!("wayland flush: {e}")))?;
            queue
                .dispatch_pending(state)
                .map_err(|e| Error::Io(format!("wayland dispatch: {e}")))?;
            let Some(guard) = queue.prepare_read() else {
                continue;
            };
            let (conn_ready, wake_ready) = {
                let conn_fd = guard.connection_fd();
                let mut fds = [
                    PollFd::new(&conn_fd, PollFlags::IN),
                    PollFd::new(&*wake, PollFlags::IN),
                ];
                rustix::event::poll(&mut fds, None).map_err(|e| Error::Io(format!("poll: {e}")))?;
                (
                    fds[0].revents().intersects(PollFlags::IN | PollFlags::HUP),
                    fds[1].revents().intersects(PollFlags::IN),
                )
            };
            if conn_ready {
                guard
                    .read()
                    .map_err(|e| Error::Io(format!("wayland read: {e}")))?;
            } else {
                drop(guard);
            }
            if wake_ready {
                let mut buf = [0u8; 64];
                while wake.read(&mut buf).is_ok_and(|n| n > 0) {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_socket_wins_and_locks_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("wayland-0");
        let new = dir.path().join("wayland-1");
        std::fs::write(&old, b"").unwrap();
        std::fs::write(&new, b"").unwrap();
        std::fs::write(dir.path().join("wayland-1.lock"), b"").unwrap();
        std::fs::write(dir.path().join("wayland-x"), b"").unwrap();
        let then = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        std::fs::File::open(&old)
            .unwrap()
            .set_modified(then)
            .unwrap();
        std::fs::File::open(&new)
            .unwrap()
            .set_modified(then + Duration::from_secs(60))
            .unwrap();
        assert_eq!(select_wayland_socket(None, dir.path()), Some(new.clone()));
        // The environment wins when set, relative to the runtime directory.
        assert_eq!(
            select_wayland_socket(Some(OsStr::new("wayland-0")), dir.path()),
            Some(old)
        );
        assert_eq!(
            select_wayland_socket(Some(OsStr::new("/tmp/w")), dir.path()),
            Some(PathBuf::from("/tmp/w"))
        );
        assert_eq!(
            select_wayland_socket(Some(OsStr::new("")), dir.path()),
            Some(new)
        );
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(select_wayland_socket(None, empty.path()), None);
    }

    #[test]
    fn text_mime_preference() {
        let offered = vec![
            "image/png".to_owned(),
            "text/plain".to_owned(),
            "TEXT/PLAIN;charset=utf-8".to_owned(),
        ];
        assert_eq!(pick_text_mime(&offered), Some("TEXT/PLAIN;charset=utf-8"));
        assert_eq!(pick_text_mime(&["image/png".to_owned()]), None);
    }

    #[test]
    fn helper_messages_round_trip() {
        let mut m = HelperMessage::new(types::SET);
        m.parts.push(HelperPart {
            mime: "text/plain".to_owned(),
            bytes: Bytes(b"hi".to_vec()),
        });
        m.promises.push(HelperPromise {
            mime: "image/png".to_owned(),
            id: "p1".to_owned(),
        });
        let bytes = frame::encode(&m, HELPER_MAX_FRAME).unwrap();
        let back: HelperMessage = frame::decode(&bytes[4..]).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn file_uris_round_trip_and_a_uri_list_becomes_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a file (1).txt");
        std::fs::write(&path, b"x").unwrap();
        let uri = file_uri(&path);
        assert!(uri.starts_with("file:///"));
        assert!(uri.contains("a%20file%20%281%29.txt"));
        assert_eq!(local_path(&uri), Some(path.clone()));
        assert_eq!(local_path("https://example.org/x"), None);
        let list = format!("{uri}\r\nhttps://example.org/not-a-file\r\n");
        let value = value_from_parts(vec![
            HelperPart {
                mime: "text/plain".to_owned(),
                bytes: Bytes(b"a file (1).txt".to_vec()),
            },
            HelperPart {
                mime: "text/uri-list".to_owned(),
                bytes: Bytes(list.into_bytes()),
            },
        ]);
        assert_eq!(value.text.as_deref(), Some("a file (1).txt"));
        assert_eq!(value.files.len(), 1);
        assert_eq!(value.files[0].name, "a file (1).txt");
        assert!(value.parts.is_empty());
    }
}
