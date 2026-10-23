//! The macOS general pasteboard behind a safe API (milestone 5, LP-108).
//!
//! `ligatured` forbids `unsafe`, and a lazy pasteboard data provider is an
//! Objective-C class that needs it to be initialised and marked for any
//! thread, so that code lives here. The daemon reads the pasteboard's value
//! through [`read`], writes one through [`write`] with a provider that is
//! called when an application asks for a lazily offered type, and polls
//! [`change_count`]; its main thread runs the Cocoa run loop through
//! [`run_main_loop`], which the pasteboard's requests need. On any platform
//! but macOS the crate has no items.

#[cfg(target_os = "macos")]
mod pasteboard;

#[cfg(target_os = "macos")]
pub use pasteboard::{
    Provided, Provider, ReadValue, WriteItem, change_count, read, run_main_loop, write,
};
