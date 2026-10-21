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
mod logging;
mod node;
mod notifications;
mod pairing;

pub use clipboard::HostClipboard;
pub use node::{Config, Device, Node, Status};
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
    /// Shows, or updates, a copy of another device's notification. Returns
    /// whether the platform accepted it.
    fn show_notification(&self, copy: NotificationCopy) -> bool;
    /// Clears the copy of `origin`'s notification `id`, if shown.
    fn dismiss_notification_copy(&self, origin: String, id: String) -> bool;
    /// Cancels this device's own notification `id`: another device dismissed
    /// its copy. Returns whether the listener could.
    fn dismiss_own_notification(&self, id: String) -> bool;
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
