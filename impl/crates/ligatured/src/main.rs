//! `ligatured`: the Ligature Protocol Linux user daemon.
//!
//! Runs as a systemd user service, owns the device's keys and MLS state,
//! performs discovery, and exposes a D-Bus interface for desktop integrations
//! and for `lpctl`. See `systemd/ligatured.service` next to this crate.
//!
//! **Status:** scaffold. Starts and exits.

fn main() {
    println!(
        "ligatured {} (scaffold; no protocol logic yet)",
        env!("CARGO_PKG_VERSION")
    );
}
