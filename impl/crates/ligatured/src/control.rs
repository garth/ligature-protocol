//! The control channel between `lpctl` and the daemon. Milestone 1 decision
//! LP-30; `schemas/control.schema.json`. Implementation only, not protocol.
//!
//! A Unix domain socket at `<runtime-dir>/control.sock`, directory 0700 and
//! socket 0600, peer uid checked on accept. Frames are the same 4-byte length
//! plus CBOR map as on QUIC streams. Each request gets exactly one response;
//! events for a request may precede it; requests on one connection are
//! answered in order.

use std::path::Path;
use std::sync::Arc;

use lp_core::frame::{self, MAX_FRAME};
use lp_core::wire::TypeName;
use lp_core::{Error, PublicKey};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};

use crate::daemon::Daemon;

/// Type names of the control messages.
pub mod types {
    /// Prefix of every control type.
    pub const PREFIX: &str = "org.ligatureprotocol.control.";
    /// The response type.
    pub const RESPONSE: &str = "org.ligatureprotocol.control.response";
    /// Create this person's keys on a fresh daemon.
    pub const INIT: &str = "org.ligatureprotocol.control.init";
    /// The daemon's status.
    pub const STATUS: &str = "org.ligatureprotocol.control.status";
    /// The trust store's devices.
    pub const DEVICES: &str = "org.ligatureprotocol.control.devices";
    /// Live capabilities of connected peers.
    pub const CAPS: &str = "org.ligatureprotocol.control.caps";
    /// Revoke a key.
    pub const REVOKE: &str = "org.ligatureprotocol.control.revoke";
    /// The grants this device issued.
    pub const GRANTS: &str = "org.ligatureprotocol.control.grants";
    /// The local clipboard's text (diagnostics and the loopback run).
    pub const CLIPBOARD_GET: &str = "org.ligatureprotocol.control.clipboard.get";
    /// Set the local clipboard's text (diagnostics and the loopback run).
    pub const CLIPBOARD_SET: &str = "org.ligatureprotocol.control.clipboard.set";
    /// Post a test notification from this device to every displaying peer.
    pub const NOTIFY: &str = "org.ligatureprotocol.control.notify";
    /// Start a ceremony as the introducer.
    pub const PAIR_INTRODUCE: &str = "org.ligatureprotocol.control.pair.introduce";
    /// Start a ceremony as the newcomer.
    pub const PAIR_JOIN: &str = "org.ligatureprotocol.control.pair.join";
    /// The user says the codes match.
    pub const PAIR_CONFIRM: &str = "org.ligatureprotocol.control.pair.confirm";
    /// The user stops the ceremony.
    pub const PAIR_ABORT: &str = "org.ligatureprotocol.control.pair.abort";
    /// Event: the offer to render as a QR code.
    pub const PAIR_OFFER: &str = "org.ligatureprotocol.control.pair.offer";
    /// Event: the six-digit code.
    pub const PAIR_CODE: &str = "org.ligatureprotocol.control.pair.code";
    /// Push files to one of this person's devices.
    pub const FILES_SEND: &str = "org.ligatureprotocol.control.files.send";
    /// Fetch files a device offered, by id.
    pub const FILES_GET: &str = "org.ligatureprotocol.control.files.get";
    /// Offer a file by id until withdrawn.
    pub const FILES_OFFER: &str = "org.ligatureprotocol.control.files.offer";
    /// The standing offers.
    pub const FILES_OFFERS: &str = "org.ligatureprotocol.control.files.offers";
    /// Withdraw an offer.
    pub const FILES_WITHDRAW: &str = "org.ligatureprotocol.control.files.withdraw";
    /// Cancel a transfer in flight.
    pub const FILES_CANCEL: &str = "org.ligatureprotocol.control.files.cancel";
    /// The transfers in flight.
    pub const FILES_TRANSFERS: &str = "org.ligatureprotocol.control.files.transfers";
    /// Event: a transfer's progress, before the response to `files.send` or
    /// `files.get`.
    pub const FILES_PROGRESS: &str = "org.ligatureprotocol.control.files.progress";
}

/// A request from `lpctl`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// The request type.
    #[serde(rename = "type")]
    pub request_type: TypeName,
    /// Chosen by the client, unique on the connection.
    pub id: u64,
    /// `revoke`: the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<PublicKey>,
    /// `pair.introduce`: the scanned offer text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<String>,
    /// `pair.introduce`: whether the newcomer becomes an introducer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub introducer: Option<bool>,
    /// `pair.confirm`/`pair.abort`: the open pairing request's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing: Option<u64>,
    /// `init`: the person's display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `init`: this device's display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    /// `clipboard.set`: the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// `notify`: the title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `notify`: the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// `files.send`, `files.offer`: absolute paths of the files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    /// `files.get`: the ids to fetch; `files.withdraw`: the one id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    /// `files.get`: an absolute directory or file path to land in, instead
    /// of the download folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// `files.cancel`: the transfer id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer: Option<String>,
}

impl Request {
    /// A request of `t` with id `id` and no fields.
    #[must_use]
    pub fn new(t: &str, id: u64) -> Self {
        Self {
            request_type: TypeName::new(t),
            id,
            ..Self::default()
        }
    }
}

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// One of the schema's codes.
    pub code: String,
    /// For a person.
    pub message: String,
}

impl ErrorBody {
    /// Maps a daemon error to a code and message.
    #[must_use]
    pub fn from_error(e: &Error) -> Self {
        let code = match e {
            Error::NotInitialised => "not-initialised",
            Error::AlreadyInitialised => "already-initialised",
            Error::Decode(_) | Error::InvalidKey(_) => "bad-request",
            Error::InvalidSignature(_) | Error::Chain(_) => "mismatch",
            _ => "internal",
        };
        Self {
            code: code.to_owned(),
            message: e.to_string(),
        }
    }

    /// A body with `code` and `message`.
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

/// The one response to a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Always [`types::RESPONSE`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// The request's id.
    pub id: u64,
    /// The result on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<ciborium::Value>,
    /// The failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl Response {
    /// A success carrying `value`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if `value` cannot be represented.
    pub fn ok<T: Serialize>(id: u64, value: &T) -> Result<Self, Error> {
        let ok = ciborium::Value::serialized(value).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(Self {
            message_type: TypeName::new(types::RESPONSE),
            id,
            ok: Some(ok),
            error: None,
        })
    }

    /// A failure.
    #[must_use]
    pub fn error(id: u64, body: ErrorBody) -> Self {
        Self {
            message_type: TypeName::new(types::RESPONSE),
            id,
            ok: None,
            error: Some(body),
        }
    }

    /// Decodes the `ok` value as `T`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if there is no `ok` or it is not a `T`.
    pub fn into_ok<T: DeserializeOwned>(self) -> Result<T, Error> {
        match (self.ok, self.error) {
            (Some(v), _) => v.deserialized().map_err(|e| Error::Decode(e.to_string())),
            (None, Some(e)) => Err(Error::Decode(format!("{}: {}", e.code, e.message))),
            (None, None) => Err(Error::Decode("response without ok or error".to_owned())),
        }
    }
}

/// An event the daemon sends before a request's response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// [`types::PAIR_OFFER`], [`types::PAIR_CODE`] or [`types::FILES_PROGRESS`].
    #[serde(rename = "type")]
    pub event_type: TypeName,
    /// The request's id.
    pub id: u64,
    /// `pair.offer`: the text to render as a QR code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<String>,
    /// `pair.code`: the six digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// `files.progress`: the transfer so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<crate::files_sync::TransferProgress>,
}

/// Writes one frame to an async writer.
///
/// # Errors
///
/// Returns [`Error::Decode`] for an unencodable message and [`Error::Io`] on
/// the socket.
pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    message: &T,
) -> Result<(), Error> {
    let bytes = frame::encode(message, MAX_FRAME)?;
    w.write_all(&bytes)
        .await
        .map_err(|e| Error::Io(format!("control write: {e}")))
}

/// Reads one frame body from an async reader; `None` at a clean end.
///
/// # Errors
///
/// Returns [`Error::Decode`] for an oversize frame and [`Error::Io`] on the
/// socket.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>, Error> {
    let mut prefix = [0u8; 4];
    match r.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(Error::Io(format!("control read: {e}"))),
    }
    let len = frame::body_len(prefix, MAX_FRAME)?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)
        .await
        .map_err(|e| Error::Io(format!("control read: {e}")))?;
    Ok(Some(body))
}

/// One device as `devices` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEntry {
    /// The device key.
    pub key: PublicKey,
    /// `binding` or `principal`.
    pub role: String,
    /// For a binding: whether it holds the self-signing key.
    pub introducer: bool,
    /// From its device record, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// From its device record, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// Whether a revocation names it.
    pub revoked: bool,
    /// Whether it is connected right now.
    pub connected: bool,
    /// Whether it is this device.
    pub this_device: bool,
    /// For a binding: the principal of the device it lives on, from its
    /// device record, so a list can group the two keys of one device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<PublicKey>,
}

/// One device as a person sees it: its binding and its principal together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceGroup {
    /// From whichever record names it.
    pub display_name: Option<String>,
    /// From whichever record names it.
    pub platform: Option<String>,
    /// My account on it, if any.
    pub binding: Option<DeviceEntry>,
    /// The device's own key, if known.
    pub principal: Option<DeviceEntry>,
}

impl DeviceGroup {
    /// Whether any of its keys is this device's.
    #[must_use]
    pub fn this_device(&self) -> bool {
        self.keys().any(|e| e.this_device)
    }

    /// Whether the binding is connected.
    #[must_use]
    pub fn connected(&self) -> bool {
        self.binding.as_ref().is_some_and(|b| b.connected)
    }

    /// Whether the binding is an introducer.
    #[must_use]
    pub fn introducer(&self) -> bool {
        self.binding.as_ref().is_some_and(|b| b.introducer)
    }

    /// Whether any of its keys is revoked.
    #[must_use]
    pub fn revoked(&self) -> bool {
        self.keys().any(|e| e.revoked)
    }

    /// Its keys, binding first.
    pub fn keys(&self) -> impl Iterator<Item = &DeviceEntry> {
        self.binding.iter().chain(self.principal.iter())
    }
}

/// Groups the entries of [`devices`] into one group per device: a binding
/// joins the principal its record names; a principal nobody binds to, or a
/// binding whose record names no principal, stands alone. Order follows the
/// entries.
#[must_use]
pub fn group_devices(entries: &[DeviceEntry]) -> Vec<DeviceGroup> {
    let mut groups: Vec<DeviceGroup> = Vec::new();
    for entry in entries {
        match entry.role.as_str() {
            "binding" => {
                let joined = entry.principal.and_then(|p| {
                    groups.iter_mut().find(|g| {
                        g.binding.is_none() && g.principal.as_ref().is_some_and(|e| e.key == p)
                    })
                });
                match joined {
                    Some(g) => {
                        g.binding = Some(entry.clone());
                        if g.display_name.is_none() {
                            g.display_name.clone_from(&entry.display_name);
                            g.platform.clone_from(&entry.platform);
                        }
                    }
                    None => groups.push(DeviceGroup {
                        display_name: entry.display_name.clone(),
                        platform: entry.platform.clone(),
                        binding: Some(entry.clone()),
                        principal: None,
                    }),
                }
            }
            _ => {
                let joined = groups.iter_mut().find(|g| {
                    g.principal.is_none()
                        && g.binding.as_ref().and_then(|b| b.principal) == Some(entry.key)
                });
                match joined {
                    Some(g) => g.principal = Some(entry.clone()),
                    None => groups.push(DeviceGroup {
                        display_name: entry.display_name.clone(),
                        platform: entry.platform.clone(),
                        binding: None,
                        principal: Some(entry.clone()),
                    }),
                }
            }
        }
    }
    groups
}

/// The `devices` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Devices {
    /// Every device the person's facts name, newest role first.
    pub devices: Vec<DeviceEntry>,
}

/// The daemon's devices, from its own verified role facts.
///
/// # Errors
///
/// Returns [`Error::NotInitialised`] or a store error.
pub fn devices(daemon: &Daemon) -> Result<Devices, Error> {
    use lp_core::facts::{DEVICE_TYPE, ROLE_TYPE, Role};
    use lp_identity::facts;
    let owner = daemon.person().ok_or(Error::NotInitialised)?;
    let store = daemon.store();
    let mut me = None;
    for stored in store.about(&owner)? {
        if stored.envelope.fact_type.as_str() == lp_core::facts::PERSON_TYPE
            && let Ok(p) = facts::verify_person(&stored.envelope)
            && me
                .as_ref()
                .is_none_or(|m: &lp_core::facts::PersonFact| p.version > m.version)
        {
            me = Some(p);
        }
    }
    let me = me.ok_or_else(|| Error::Chain("my person fact is missing".to_owned()))?;
    let connected = daemon.manager().peers();
    let mut out = Vec::new();
    for stored in store.of_type(ROLE_TYPE)? {
        let Ok(role) = facts::verify_role(&stored.envelope, &me) else {
            continue;
        };
        let role_name = match role.role {
            Role::Binding => "binding",
            Role::Principal => "principal",
            Role::Person => continue,
        };
        if out.iter().any(|d: &DeviceEntry| d.key == role.subject) {
            continue;
        }
        let record = store
            .about(&role.subject)?
            .into_iter()
            .filter(|s| s.envelope.fact_type.as_str() == DEVICE_TYPE)
            .filter_map(|s| facts::verify_device(&s.envelope).ok())
            .max_by_key(|d| d.created_at);
        out.push(DeviceEntry {
            key: role.subject,
            role: role_name.to_owned(),
            introducer: role.introducer,
            display_name: record.as_ref().map(|d| d.display_name.clone()),
            platform: record.as_ref().map(|d| d.platform.clone()),
            revoked: store.is_revoked(&role.subject)?,
            connected: connected.contains(&role.subject),
            this_device: role.subject == *daemon.binding().public()
                || role.subject == *daemon.principal().public(),
            principal: record.as_ref().and_then(|d| d.principal),
        });
    }
    out.reverse();
    Ok(Devices { devices: out })
}

/// Revokes `key` with this device's self-signing key and drops its connection.
///
/// # Errors
///
/// Returns [`Error::NotInitialised`], [`Error::Chain`] if this device is not
/// an introducer, or a store error.
pub fn revoke(daemon: &Daemon, key: &PublicKey) -> Result<(), Error> {
    let owner = daemon.person().ok_or(Error::NotInitialised)?;
    let self_signing = daemon.self_signing()?.ok_or_else(|| {
        Error::Chain("this device is not an introducer and cannot revoke".to_owned())
    })?;
    let fact = lp_core::facts::RevocationFact {
        subject: *key,
        kind: lp_core::facts::RevocationKind::Revocation,
        person: owner,
        revoked_at: lp_core::wire::Timestamp::now(),
    };
    let env = lp_identity::facts::sign(&self_signing, &fact)?;
    daemon.store().insert(&env)?;
    daemon.manager().drop_peer(key);
    tracing::info!(%key, "revoked");
    Ok(())
}

/// Revokes and then pushes the revocation to every connected peer.
///
/// # Errors
///
/// As [`revoke`].
pub async fn revoke_and_push(daemon: &Arc<Daemon>, key: &PublicKey) -> Result<(), Error> {
    revoke(daemon, key)?;
    daemon.gossip_all().await;
    Ok(())
}

/// Serves the control socket at `path` until the listener fails. Stale sockets
/// are removed; the directory is made 0700 and the socket 0600.
///
/// # Errors
///
/// Returns [`Error::Io`] if the socket cannot be bound.
pub fn serve(daemon: Arc<Daemon>, path: &Path) -> Result<tokio::task::JoinHandle<()>, Error> {
    let listener = bind(path)?;
    Ok(tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(%e, "control socket accept failed");
                    break;
                }
            };
            if !same_user(&stream) {
                tracing::warn!("control connection from another user refused");
                continue;
            }
            let daemon = daemon.clone();
            tokio::spawn(async move {
                if let Err(e) = connection(daemon, stream).await {
                    tracing::debug!(%e, "control connection ended");
                }
            });
        }
    }))
}

fn bind(path: &Path) -> Result<UnixListener, Error> {
    let io = |e: std::io::Error| Error::Io(format!("{}: {e}", path.display()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(io)?;
        }
    }
    if path.exists() {
        // Stale if nobody answers.
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Err(Error::Io(format!(
                "{}: another daemon is listening",
                path.display()
            )));
        }
        std::fs::remove_file(path).map_err(io)?;
    }
    let listener = UnixListener::bind(path).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
    }
    tracing::info!(socket = %path.display(), "control socket ready");
    Ok(listener)
}

fn same_user(stream: &UnixStream) -> bool {
    match stream.peer_cred() {
        Ok(cred) => {
            #[cfg(unix)]
            {
                // SAFETY-free: getuid has no preconditions and cannot fail.
                let me = unsafe_free_uid();
                cred.uid() == me
            }
        }
        Err(_) => false,
    }
}

#[cfg(unix)]
fn unsafe_free_uid() -> u32 {
    // std exposes no getuid; the metadata of a file we own does. The data
    // directory is created by this process, so its owner is us.
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata("/proc/self")
        .or_else(|_| std::env::current_exe().and_then(std::fs::metadata))
        .map(|m| m.uid())
        .unwrap_or(u32::MAX)
}

/// Per-connection state: open pairings by request id.
#[derive(Default)]
pub struct ConnectionState {
    /// Pairing sessions keyed by the request id that opened them.
    pub pairings: std::collections::HashMap<u64, crate::ceremony::PairingHandle>,
}

/// Something to write to the client.
#[derive(Debug)]
pub enum Outgoing {
    /// An event for an open request.
    Event(Event),
    /// A request's response.
    Response(Response),
}

async fn connection(daemon: Arc<Daemon>, stream: UnixStream) -> Result<(), Error> {
    let (mut reader, mut writer) = stream.into_split();
    let (out, mut outgoing) = tokio::sync::mpsc::channel::<Outgoing>(32);
    let writer_task = tokio::spawn(async move {
        while let Some(msg) = outgoing.recv().await {
            let result = match &msg {
                Outgoing::Event(e) => write_frame(&mut writer, e).await,
                Outgoing::Response(r) => write_frame(&mut writer, r).await,
            };
            if result.is_err() {
                break;
            }
        }
    });
    let mut state = ConnectionState::default();
    while let Some(body) = read_frame(&mut reader).await? {
        let request: Request = match frame::decode(&body) {
            Ok(r) => r,
            Err(e) => {
                let id = frame::decode::<serde_json_like::IdOnly>(&body)
                    .map(|i| i.id)
                    .unwrap_or(0);
                let _ = out
                    .send(Outgoing::Response(Response::error(
                        id,
                        ErrorBody::new("bad-request", e.to_string()),
                    )))
                    .await;
                continue;
            }
        };
        if let Some(response) = handle(&daemon, &mut state, request, out.clone()).await
            && out.send(Outgoing::Response(response)).await.is_err()
        {
            break;
        }
    }
    drop(out);
    let _ = writer_task.await;
    Ok(())
}

mod serde_json_like {
    #[derive(serde::Deserialize)]
    pub struct IdOnly {
        #[serde(default)]
        pub id: u64,
    }
}

/// Handles one request. Returns the response, or `None` when a task will
/// write it later (pairing, handled by [`crate::ceremony`]).
pub async fn handle(
    daemon: &Arc<Daemon>,
    state: &mut ConnectionState,
    request: Request,
    out: tokio::sync::mpsc::Sender<Outgoing>,
) -> Option<Response> {
    let id = request.id;
    let result: Result<Response, Error> = match request.request_type.as_str() {
        types::STATUS => Response::ok(id, &daemon.status()),
        types::INIT => match request.name.as_deref() {
            Some(name) if !name.trim().is_empty() => daemon
                .init(name.trim(), request.device_name.as_deref())
                .and_then(|person| Response::ok(id, &InitResult { person })),
            _ => Ok(Response::error(
                id,
                ErrorBody::new("bad-request", "init needs a name"),
            )),
        },
        types::DEVICES => devices(daemon).and_then(|d| Response::ok(id, &d)),
        types::GRANTS => crate::grants::list(daemon).and_then(|g| Response::ok(id, &g)),
        types::CLIPBOARD_GET => match daemon.clipboard() {
            Some(c) => c.read_text().and_then(|text| {
                Response::ok(
                    id,
                    &ClipboardText {
                        available: c.available(),
                        text,
                    },
                )
            }),
            None => Ok(Response::error(
                id,
                ErrorBody::new("not-found", "no clipboard on this daemon"),
            )),
        },
        types::CLIPBOARD_SET => match (daemon.clipboard(), request.text.as_deref()) {
            (Some(c), Some(text)) => c.set_text(text).and_then(|()| Response::ok(id, &Empty {})),
            (None, _) => Ok(Response::error(
                id,
                ErrorBody::new("not-found", "no clipboard on this daemon"),
            )),
            (_, None) => Ok(Response::error(
                id,
                ErrorBody::new("bad-request", "clipboard.set needs text"),
            )),
        },
        types::NOTIFY => match (daemon.notification_sync(), request.title.as_deref()) {
            (Some(sync), Some(title)) => {
                let notification = crate::notifications::Notification {
                    id: format!("lpctl-{}", lp_core::wire::Timestamp::now().unix()),
                    app_id: crate::notification_sync::LPCTL_APP.to_owned(),
                    app_name: "lpctl".to_owned(),
                    title: Some(title.to_owned()),
                    body: request.body.clone(),
                    posted_at: lp_core::wire::Timestamp::now(),
                    icon: None,
                };
                sync.local_posted(&notification).await;
                Response::ok(id, &Empty {})
            }
            (None, _) => Ok(Response::error(
                id,
                ErrorBody::new("not-found", "no notifications on this daemon"),
            )),
            (_, None) => Ok(Response::error(
                id,
                ErrorBody::new("bad-request", "notify needs a title"),
            )),
        },
        types::CAPS => Response::ok(
            id,
            &Caps {
                local: daemon.registry().local(),
                peers: daemon.registry().peers(),
            },
        ),
        types::REVOKE => match request.key {
            Some(key) => revoke_and_push(daemon, &key)
                .await
                .and_then(|()| Response::ok(id, &Empty {})),
            None => Ok(Response::error(
                id,
                ErrorBody::new("bad-request", "revoke needs a key"),
            )),
        },
        t if t.starts_with(types::PREFIX) && t.contains(".pair.") => {
            return crate::ceremony::handle_control(daemon, state, request, out).await;
        }
        t if t.starts_with(types::PREFIX) && t.contains(".files.") => {
            return handle_files(daemon, request, out);
        }
        other => Ok(Response::error(
            id,
            ErrorBody::new("unknown-type", format!("unknown request type {other}")),
        )),
    };
    Some(match result {
        Ok(r) => r,
        Err(e) => Response::error(id, ErrorBody::from_error(&e)),
    })
}

/// Handles a `files.*` request. A send or a get is answered by a task that
/// first streams progress events; the rest answer at once.
fn handle_files(
    daemon: &Arc<Daemon>,
    request: Request,
    out: tokio::sync::mpsc::Sender<Outgoing>,
) -> Option<Response> {
    use crate::files::Destination;
    use crate::files_sync::FilesSync;
    let id = request.id;
    let Some(files) = daemon.files_sync() else {
        return Some(Response::error(
            id,
            ErrorBody::new("not-found", "no files capability on this daemon"),
        ));
    };
    let bad = |message: &str| Some(Response::error(id, ErrorBody::new("bad-request", message)));
    let result: Result<Response, Error> = match request.request_type.as_str() {
        types::FILES_SEND => {
            let (Some(peer), Some(paths)) = (request.key, request.paths.as_deref()) else {
                return bad("files.send needs a key and paths");
            };
            let mut list = Vec::new();
            for path in paths {
                match FilesSync::new_id().and_then(|fid| {
                    crate::files::FileSource::describe_path(&fid, std::path::Path::new(path))
                }) {
                    Ok(entry) => list.push(entry),
                    Err(e) => return Some(Response::error(id, ErrorBody::from_error(&e))),
                }
            }
            match files.push(&peer, list) {
                Ok(transfer) => {
                    stream_progress(id, transfer, out);
                    return None;
                }
                Err(e) => Err(e),
            }
        }
        types::FILES_GET => {
            let (Some(peer), Some(ids)) = (request.key, request.ids.clone()) else {
                return bad("files.get needs a key and ids");
            };
            let destination = request
                .to
                .as_deref()
                .map_or(Destination::Downloads, |p| Destination::Path(p.into()));
            match files.fetch(&peer, ids, destination) {
                Ok(transfer) => {
                    stream_progress(id, transfer, out);
                    return None;
                }
                Err(e) => Err(e),
            }
        }
        types::FILES_OFFER => match request.paths.as_deref().and_then(<[String]>::first) {
            Some(path) => files
                .offer_path(std::path::Path::new(path))
                .and_then(|file| Response::ok(id, &file)),
            None => return bad("files.offer needs a path"),
        },
        types::FILES_OFFERS => Response::ok(
            id,
            &Offers {
                offers: files.offers(),
                location: files.store_description(),
            },
        ),
        types::FILES_WITHDRAW => match request.ids.as_deref().and_then(<[String]>::first) {
            Some(fid) if files.withdraw(fid) => Response::ok(id, &Empty {}),
            Some(fid) => Ok(Response::error(
                id,
                ErrorBody::new("not-found", format!("no standing offer {fid}")),
            )),
            None => return bad("files.withdraw needs an id"),
        },
        types::FILES_CANCEL => match request.transfer.as_deref() {
            Some(t) if files.cancel(t) => Response::ok(id, &Empty {}),
            Some(t) => Ok(Response::error(
                id,
                ErrorBody::new("not-found", format!("no transfer {t} in flight")),
            )),
            None => return bad("files.cancel needs a transfer"),
        },
        types::FILES_TRANSFERS => Response::ok(
            id,
            &Transfers {
                transfers: files.transfers(),
            },
        ),
        other => Ok(Response::error(
            id,
            ErrorBody::new("unknown-type", format!("unknown request type {other}")),
        )),
    };
    Some(match result {
        Ok(r) => r,
        Err(e) => Response::error(id, ErrorBody::from_error(&e)),
    })
}

/// Sends a `files.progress` event for every change of `transfer`, then its
/// final state as the response to request `id`.
fn stream_progress(
    id: u64,
    transfer: Arc<crate::files_sync::Transfer>,
    out: tokio::sync::mpsc::Sender<Outgoing>,
) {
    tokio::spawn(async move {
        let mut rx = transfer.watch();
        loop {
            let progress = rx.borrow_and_update().clone();
            let finished = progress.finished.is_some();
            if finished {
                let response = Response::ok(id, &progress)
                    .unwrap_or_else(|e| Response::error(id, ErrorBody::from_error(&e)));
                let _ = out.send(Outgoing::Response(response)).await;
                return;
            }
            let event = Event {
                event_type: TypeName::new(types::FILES_PROGRESS),
                id,
                offer: None,
                code: None,
                progress: Some(progress),
            };
            if out.send(Outgoing::Event(event)).await.is_err() {
                // The client went away: the transfer carries on unwatched.
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    });
}

/// The `files.offers` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offers {
    /// The standing offers.
    pub offers: Vec<lp_core::files::FileDescriptor>,
    /// Where pushed files land on this device.
    pub location: String,
}

/// The `files.transfers` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfers {
    /// Every transfer in flight.
    pub transfers: Vec<crate::files_sync::TransferProgress>,
}

/// The `init` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitResult {
    /// The new person's master key.
    pub person: PublicKey,
}

/// The `caps` result: the live view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caps {
    /// What this device offers.
    pub local: Vec<lp_core::capability::Descriptor>,
    /// Per connected peer, its newest manifest.
    pub peers: Vec<crate::capabilities::PeerCaps>,
}

/// The `clipboard.get` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardText {
    /// Whether the clipboard is reachable.
    pub available: bool,
    /// Its text, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// An empty result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

/// A client of the control socket, for `lpctl` and tests.
#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    next_id: u64,
}

impl Client {
    /// Connects to the socket at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if nobody is listening.
    pub async fn connect(path: &Path) -> Result<Self, Error> {
        let stream = UnixStream::connect(path)
            .await
            .map_err(|e| Error::Io(format!("{}: {e} (is ligatured running?)", path.display())))?;
        Ok(Self { stream, next_id: 1 })
    }

    /// Sends a request and returns its response. Events for it go to
    /// `on_event`, which may answer with a follow-up request (such as
    /// `pair.confirm`) that is sent on the same connection; the follow-up's
    /// own response is checked for an error and otherwise skipped.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] or [`Error::Decode`] on the channel; a daemon
    /// error is inside the response.
    pub async fn call(
        &mut self,
        mut request: Request,
        mut on_event: impl FnMut(&Event) -> Option<Request>,
    ) -> Result<Response, Error> {
        request.id = self.next_id;
        self.next_id += 1;
        write_frame(&mut self.stream, &request).await?;
        loop {
            let body = read_frame(&mut self.stream)
                .await?
                .ok_or_else(|| Error::Io("daemon closed the control connection".to_owned()))?;
            let t = frame::peek_type(&body)?;
            if t.as_str() == types::RESPONSE {
                let response: Response = frame::decode(&body)?;
                if response.id == request.id {
                    return Ok(response);
                }
                if let Some(e) = &response.error {
                    return Err(Error::Decode(format!(
                        "follow-up request failed: {}: {}",
                        e.code, e.message
                    )));
                }
                continue;
            }
            let event: Event = frame::decode(&body)?;
            if let Some(mut follow_up) = on_event(&event) {
                follow_up.id = self.next_id;
                self.next_id += 1;
                write_frame(&mut self.stream, &follow_up).await?;
            }
        }
    }

    /// Sends a request with no interest in events and decodes its `ok`.
    ///
    /// # Errors
    ///
    /// Returns the daemon's error as [`Error::Decode`] with its code.
    pub async fn request<T: DeserializeOwned>(&mut self, request: Request) -> Result<T, Error> {
        self.call(request, |_| None).await?.into_ok()
    }
}
