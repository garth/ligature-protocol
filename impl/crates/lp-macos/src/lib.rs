//! The macOS general pasteboard (milestone 5, LP-108) and user notification
//! centre (LP-94) behind a safe API.
//!
//! `ligatured` forbids `unsafe`, and a lazy pasteboard data provider and the
//! notification centre's delegate are Objective-C classes that need it to be
//! initialised and marked for any thread, so that code lives here. The daemon
//! reads the pasteboard's value through `read`, writes one through `write`
//! with a provider that is called when an application asks for a lazily
//! offered type, and polls `change_count`; its main thread runs the Cocoa run
//! loop through `run_main_loop`, which the pasteboard's requests and the
//! notification centre's callbacks need. Its notification backend posts and
//! removes notifications through `notifications`, which works only inside an
//! application bundle. On any platform but macOS the crate has no items, so
//! the names are not links.

#[cfg(target_os = "macos")]
mod pasteboard;

#[cfg(target_os = "macos")]
pub use pasteboard::{
    Provided, Provider, ReadValue, WriteItem, change_count, read, run_main_loop, write,
};

/// The user notification centre: posting, replacing and removing the
/// daemon's notifications and learning what the user did with them.
#[cfg(target_os = "macos")]
pub mod notifications;
