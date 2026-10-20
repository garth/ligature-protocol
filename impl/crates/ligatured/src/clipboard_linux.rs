//! The Linux clipboard over Wayland `wlr-data-control`/`ext-data-control`.
//! Milestone 1 decisions LP-29 and LP-34.
//!
//! `wl-clipboard-rs` connects through `WAYLAND_DISPLAY`, and a daemon started
//! by systemd may not have that variable, or may start before the compositor.
//! Setting a process-wide environment variable at runtime is unsafe in Rust,
//! so the daemon runs the Wayland side in a small helper child process
//! (`ligatured clipboard-helper`) whose environment names the socket chosen
//! here: `WAYLAND_DISPLAY` when set, else the newest `wayland-N` socket in
//! `$XDG_RUNTIME_DIR`. Parent and child talk over the child's stdio in the
//! same length-prefixed CBOR frames as everything else. When the compositor
//! goes away the helper exits and the supervisor retries every few seconds;
//! while no helper runs the clipboard is unavailable and not advertised.

use std::ffi::{OsStr, OsString};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lp_core::Error;
use lp_core::frame;
use lp_core::wire::TypeName;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::clipboard::{Change, Clipboard};

/// How often the supervisor looks for a compositor when there is none.
pub const RETRY: Duration = Duration::from_secs(3);

/// Type names of the helper's messages.
pub mod types {
    /// Helper to daemon: connected and watching.
    pub const READY: &str = "org.ligatureprotocol.clipboard-helper.ready";
    /// Helper to daemon: the clipboard now holds this text.
    pub const CHANGED: &str = "org.ligatureprotocol.clipboard-helper.changed";
    /// Daemon to helper: put this text on the clipboard.
    pub const SET: &str = "org.ligatureprotocol.clipboard-helper.set";
}

/// A message between the daemon and its clipboard helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperMessage {
    /// One of [`types`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// The text, for `changed` and `set`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl HelperMessage {
    fn new(t: &str, text: Option<String>) -> Self {
        Self {
            message_type: TypeName::new(t),
            text,
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
    stdin: Mutex<std::process::ChildStdin>,
}

/// The Linux clipboard: a supervised helper process. Implements
/// [`Clipboard`] with a stable change channel across helper restarts.
pub struct LinuxClipboard {
    helper: Mutex<Option<Helper>>,
    last: Arc<Mutex<Option<String>>>,
    changes: broadcast::Sender<Change>,
    runtime_dir: PathBuf,
    env_display: Option<OsString>,
}

impl std::fmt::Debug for LinuxClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LinuxClipboard(available={})", self.available())
    }
}

impl LinuxClipboard {
    /// Creates the clipboard for the current environment and starts its
    /// supervisor, which starts a helper whenever a compositor socket exists
    /// and no helper runs.
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
            changes,
            runtime_dir,
            env_display,
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
        *self.lock_helper() = Some(Helper {
            child,
            stdin: Mutex::new(stdin),
        });
        let changes = self.changes.clone();
        let last = self.last_handle();
        std::thread::spawn(move || {
            while let Ok(Some(body)) = read_frame_sync(&mut stdout) {
                let Ok(msg) = frame::decode::<HelperMessage>(&body) else {
                    continue;
                };
                if msg.message_type.as_str() == types::CHANGED {
                    if let Some(text) = msg.text {
                        *last
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some(text.clone());
                        let _ = changes.send(Change { text });
                    }
                }
            }
            tracing::info!("clipboard helper ended; clipboard unavailable until it restarts");
        });
        *self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        Ok(())
    }

    fn last_handle(&self) -> Arc<Mutex<Option<String>>> {
        Arc::clone(&self.last)
    }

    fn lock_helper(&self) -> std::sync::MutexGuard<'_, Option<Helper>> {
        self.helper
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
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
        Ok(self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }

    fn set_text(&self, text: &str) -> Result<(), Error> {
        let slot = self.lock_helper();
        let helper = slot
            .as_ref()
            .ok_or_else(|| Error::Io("clipboard unavailable".to_owned()))?;
        let bytes = frame::encode(
            &HelperMessage::new(types::SET, Some(text.to_owned())),
            frame::MAX_FRAME,
        )?;
        let mut stdin = helper
            .stdin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        stdin
            .write_all(&bytes)
            .and_then(|()| stdin.flush())
            .map_err(|e| Error::Io(format!("clipboard helper: {e}")))?;
        *self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(text.to_owned());
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }
}

fn read_frame_sync<R: std::io::Read>(r: &mut R) -> Result<Option<Vec<u8>>, Error> {
    let mut prefix = [0u8; 4];
    match r.read_exact(&mut prefix) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(Error::Io(format!("helper read: {e}"))),
    }
    let len = frame::body_len(prefix, frame::MAX_FRAME)?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)
        .map_err(|e| Error::Io(format!("helper read: {e}")))?;
    Ok(Some(body))
}

/// The helper process: watches the Wayland clipboard and serves `set`
/// requests until stdin closes. Runs in the child; never returns normally
/// while the compositor is up.
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
    write_sync(&stdout, &HelperMessage::new(types::READY, None))?;

    // stdin: set requests. EOF means the daemon went away.
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        while let Ok(Some(body)) = read_frame_sync(&mut stdin) {
            let Ok(msg) = frame::decode::<HelperMessage>(&body) else {
                continue;
            };
            if msg.message_type.as_str() == types::SET {
                if let Some(text) = msg.text {
                    std::thread::spawn(move || {
                        use wl_clipboard_rs::copy::{MimeType, Options, Source};
                        let mut options = Options::new();
                        options.foreground(true);
                        if let Err(e) = options.copy(
                            Source::Bytes(text.into_bytes().into_boxed_slice()),
                            MimeType::Text,
                        ) {
                            eprintln!("clipboard helper: copy failed: {e}");
                        }
                    });
                }
            }
        }
        cancel.cancel();
        std::process::exit(0);
    });

    loop {
        match watcher.next_event() {
            Ok(Some(ClipboardEvent::Changed {
                mime_types,
                mut offer,
                ..
            })) => {
                let Some(mime) = pick_text_mime(&mime_types) else {
                    continue;
                };
                let Ok(mut reader) = offer.receive(mime) else {
                    continue;
                };
                let mut bytes = Vec::new();
                if reader.read_to_end(&mut bytes).is_err() {
                    continue;
                }
                let text = String::from_utf8_lossy(&bytes).into_owned();
                write_sync(&stdout, &HelperMessage::new(types::CHANGED, Some(text)))?;
            }
            Ok(Some(ClipboardEvent::Cleared { .. })) => {}
            Ok(None) => return Ok(()),
            Err(e) => return Err(Error::Io(format!("wayland clipboard: {e}"))),
        }
    }
}

/// The text type to request from an offer, if it has one.
#[must_use]
pub fn pick_text_mime(offered: &[String]) -> Option<&str> {
    const PREFERRED: [&str; 5] = [
        "text/plain;charset=utf-8",
        "text/plain",
        "UTF8_STRING",
        "TEXT",
        "STRING",
    ];
    PREFERRED
        .iter()
        .find_map(|p| offered.iter().find(|m| m.eq_ignore_ascii_case(p)))
        .map(String::as_str)
}

fn write_sync(out: &Arc<Mutex<std::io::Stdout>>, msg: &HelperMessage) -> Result<(), Error> {
    let bytes = frame::encode(msg, frame::MAX_FRAME)?;
    let mut out = out
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    out.write_all(&bytes)
        .and_then(|()| out.flush())
        .map_err(|e| Error::Io(format!("helper write: {e}")))
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
        let m = HelperMessage::new(types::SET, Some("hi".to_owned()));
        let bytes = frame::encode(&m, frame::MAX_FRAME).unwrap();
        let back: HelperMessage = frame::decode(&bytes[4..]).unwrap();
        assert_eq!(back, m);
    }
}
