//! The daemon's library side: what `main` wires together and what tests
//! exercise. See the crate's `main.rs` for the binary.

pub mod capabilities;
pub mod ceremony;
pub mod clipboard;
#[cfg(target_os = "linux")]
pub mod clipboard_linux;
#[cfg(target_os = "macos")]
pub mod clipboard_macos;
pub mod clipboard_sync;
pub mod control;
pub mod daemon;
pub mod files;
pub mod files_sync;
pub mod gossip;
pub mod grants;
pub mod input;
#[cfg(target_os = "linux")]
pub mod input_linux;
pub mod notification_sync;
pub mod notifications;
#[cfg(target_os = "linux")]
pub mod notifications_linux;
#[cfg(target_os = "macos")]
pub mod notifications_macos;
pub mod run;
pub mod screen;
#[cfg(target_os = "linux")]
pub mod screen_linux;
