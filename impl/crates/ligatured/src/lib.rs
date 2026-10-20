//! The daemon's library side: what `main` wires together and what tests
//! exercise. See the crate's `main.rs` for the binary.

pub mod capabilities;
pub mod ceremony;
pub mod clipboard;
#[cfg(target_os = "linux")]
pub mod clipboard_linux;
pub mod control;
pub mod daemon;
pub mod gossip;
pub mod run;
