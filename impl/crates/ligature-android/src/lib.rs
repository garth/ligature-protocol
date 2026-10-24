//! The Ligature Protocol daemon as a library for the Android app, behind
//! UniFFI bindings. Milestone 2 (LP-77); spec chapter 08 section 3.
//!
//! The app is the host: it starts a [`Node`] in its foreground service and
//! supplies through the [`Host`] trait what Android keeps from a library,
//! namely the system clipboard, a place for log lines and a way to be told
//! that something changed. Everything else (keys, store, QUIC, discovery,
//! gossip, manifests, the ceremony, clipboard synchronisation) is the same
//! code as `ligatured`, run on a tokio runtime the node owns. There is no
//! control socket; the app calls the node directly.
//!
//! The clipboard works the other way round from the desktops: the host tells
//! the node when the clipboard changed and what it holds, because only the
//! app can read it, and the node asks the host to set it.

#![allow(clippy::needless_pass_by_value)]

mod clipboard;
mod files;
mod logging;
mod node;
mod notifications;
mod pairing;

pub use clipboard::HostClipboard;
pub use files::HostFiles;
pub use lp_core::input::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
pub use node::{Config, Device, Node, OutgoingFile, Status, TransferInfo};
pub use notifications::HostNotifications;
pub use pairing::Pairing;

use std::sync::Arc;

uniffi::setup_scaffolding!();

/// What the host is told about, beyond the answers to its own calls.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Event {
    /// The identity, the connected peers or what they offer changed: the
    /// host should read [`Node::status`] again.
    StatusChanged,
    /// A newcomer's ceremony has its offer: the text to show as a QR code.
    PairingOffer {
        /// The offer text, `ligature:pair/1?…`.
        offer: String,
    },
    /// The ceremony derived the six-digit code: show it and ask the user.
    PairingCode {
        /// Six decimal digits.
        code: String,
    },
    /// The ceremony ended.
    PairingFinished {
        /// Why it failed, or `None` when the device is now paired.
        error: Option<String>,
    },
    /// A peer's clipboard text was applied to the host's clipboard.
    ClipboardReceived,
    /// A file transfer started, moved or ended: show or update its
    /// progress notification (milestone 4).
    TransferChanged {
        /// The transfer so far.
        info: TransferInfo,
    },
    /// The drive this phone ran as a touchpad ended, by the app, the desktop
    /// or the connection (milestone 7): the touchpad screen should close.
    InputEnded {
        /// The device that was driven.
        peer: String,
    },
}

/// A device this phone can drive as a touchpad (milestone 7).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct InputTarget {
    /// Its binding key.
    pub key: String,
    /// Its display name.
    pub name: String,
    /// Whether it types a `text` event's string.
    pub text: bool,
}

/// The driven screen's size, from `ready`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct InputScreen {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// What the touchpad screen sends.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum TouchEvent {
    /// Relative motion, in the driven screen's pixels.
    Motion {
        /// Horizontal.
        dx: f64,
        /// Vertical.
        dy: f64,
    },
    /// A pointer button, by evdev code (272 is the left button).
    Button {
        /// The code.
        code: u16,
        /// Pressed or released.
        pressed: bool,
    },
    /// Scrolling, in wheel steps.
    Scroll {
        /// Horizontal steps.
        dx: f64,
        /// Vertical steps.
        dy: f64,
    },
    /// A key, by evdev code (14 is Backspace, 28 is Enter).
    Key {
        /// The code.
        code: u16,
        /// Pressed or released.
        pressed: bool,
    },
    /// Characters to type.
    Text {
        /// The characters.
        text: String,
    },
}

/// One entry of a clipboard value the node sets on the host, served by the
/// app's content provider under `key` (milestone 5): a pushed part, a
/// promised part the node fetches on first open, or a promised file.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ClipItem {
    /// The provider's key for it, the last segment of its content URI.
    pub key: String,
    /// Its content type.
    pub mime: String,
    /// Its display name.
    pub name: String,
    /// Its size in bytes.
    pub size: u64,
    /// Whether it is a file of a copied selection rather than a
    /// representation of the value.
    pub file: bool,
}

/// A clipboard value as the node sets it on the host: the text and HTML go
/// on the clip directly, the items as content URIs of the app's provider.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostClipValue {
    /// The plain text.
    pub text: Option<String>,
    /// The HTML representation.
    pub html: Option<String>,
    /// The other representations and the files.
    pub items: Vec<ClipItem>,
}

/// What the provider serves for a key: bytes, or a file to open.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ClipItemData {
    /// The bytes, for a part held in memory.
    pub bytes: Option<Vec<u8>>,
    /// A path to open, for a file or a large part in the cache.
    pub path: Option<String>,
}

/// Where the host put a download the node is about to write.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DownloadTarget {
    /// The host's handle, given back to finish or abort it.
    pub id: u64,
    /// A path the node opens for writing: `/proc/self/fd/N` for a descriptor
    /// the host holds open until the download is finished or aborted.
    pub path: String,
}

/// A published download, as the host names it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PublishedDownload {
    /// The name the platform gave it, which may differ from the one asked
    /// for when it numbered a clash.
    pub name: String,
    /// Where it is: a content URI the host can open.
    pub location: String,
}

/// A log line's level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LogLevel {
    /// `tracing::Level::ERROR`.
    Error,
    /// `tracing::Level::WARN`.
    Warn,
    /// `tracing::Level::INFO`.
    Info,
    /// `tracing::Level::DEBUG`.
    Debug,
    /// `tracing::Level::TRACE`.
    Trace,
}

/// A notification an app on this device posted, as the host's listener saw
/// it (milestone 3).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LocalNotification {
    /// Stable for the notification's life: the listener's key.
    pub id: String,
    /// The package name.
    pub app_id: String,
    /// The app's label.
    pub app_name: String,
    /// The title, if any.
    pub title: Option<String>,
    /// The text, if any.
    pub body: Option<String>,
    /// When it was posted, in seconds since the epoch.
    pub posted_at_unix: i64,
    /// The app's icon as PNG, if the host could render one.
    pub icon_png: Option<Vec<u8>>,
}

/// A copy of another device's notification for the host to show.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NotificationCopy {
    /// The emitting device's binding key.
    pub origin: String,
    /// That device's display name.
    pub origin_name: String,
    /// The id in the origin's id space; the same id again is an update.
    pub id: String,
    /// The app's identifier on the origin.
    pub app_id: String,
    /// The app's name.
    pub app_name: String,
    /// The title, if any.
    pub title: Option<String>,
    /// The text, if any.
    pub body: Option<String>,
    /// When it was posted on the origin, in seconds since the epoch.
    pub posted_at_unix: i64,
    /// The app's icon as PNG, if one travelled.
    pub icon_png: Option<Vec<u8>>,
}

/// What the app supplies to the node. Implemented in Kotlin; every method may
/// be called from any of the node's threads.
#[uniffi::export(with_foreign)]
pub trait Host: Send + Sync {
    /// Puts `text` on the system clipboard. Returns whether the platform
    /// accepted it.
    fn set_clipboard(&self, text: String) -> bool;
    /// Puts a whole value on the system clipboard: the text, the HTML and
    /// the items as content URIs the app's provider serves through
    /// [`node::Node::clipboard_item`]. Returns whether the platform accepted
    /// it.
    fn set_clipboard_value(&self, value: HostClipValue) -> bool;
    /// Shows, or updates, a copy of another device's notification. Returns
    /// whether the platform accepted it.
    fn show_notification(&self, copy: NotificationCopy) -> bool;
    /// Clears the copy of `origin`'s notification `id`, if shown.
    fn dismiss_notification_copy(&self, origin: String, id: String) -> bool;
    /// Cancels this device's own notification `id`: another device dismissed
    /// its copy. Returns whether the listener could.
    fn dismiss_own_notification(&self, id: String) -> bool;
    /// Starts a download of `size` bytes named `name` with content type
    /// `mime` in the platform's download folder, pending until finished.
    /// `None` if the platform refuses.
    fn create_download(&self, name: String, mime: String, size: u64) -> Option<DownloadTarget>;
    /// The node wrote and verified the download: publish it. `None` if the
    /// platform refuses.
    fn finish_download(&self, id: u64) -> Option<PublishedDownload>;
    /// The download will not complete: remove every trace of it.
    fn abort_download(&self, id: u64);
    /// Something happened that the user interface may want to show.
    fn on_event(&self, event: Event);
    /// A log line from the node, for logcat.
    fn log(&self, level: LogLevel, target: String, message: String);
}

/// Why a call failed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Error)]
#[uniffi(flat_error)]
pub enum LpError {
    /// The node has no identity yet: create one or pair.
    NotInitialised,
    /// The node already has an identity.
    AlreadyInitialised,
    /// A key or an offer that does not parse, or a request that is incomplete.
    BadRequest(String),
    /// A signature, a chain or a ceremony that does not check out.
    Mismatch(String),
    /// A file, the store, a socket or the platform failed.
    Internal(String),
}

impl std::fmt::Display for LpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInitialised => f.write_str("not initialised"),
            Self::AlreadyInitialised => f.write_str("already initialised"),
            Self::BadRequest(m) | Self::Mismatch(m) | Self::Internal(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for LpError {}

impl From<lp_core::Error> for LpError {
    fn from(e: lp_core::Error) -> Self {
        use lp_core::Error;
        match e {
            Error::NotInitialised => Self::NotInitialised,
            Error::AlreadyInitialised => Self::AlreadyInitialised,
            Error::Decode(_) | Error::InvalidKey(_) => Self::BadRequest(e.to_string()),
            Error::InvalidSignature(_) | Error::Chain(_) => Self::Mismatch(e.to_string()),
            _ => Self::Internal(e.to_string()),
        }
    }
}

/// The library's version, from the workspace.
#[uniffi::export]
#[must_use]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

type SharedHost = Arc<dyn Host>;
