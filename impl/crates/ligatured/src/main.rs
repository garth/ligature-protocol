//! `ligatured`: the Ligature Protocol Linux user daemon.
//!
//! Runs as a systemd user service for one account's binding (a system-level
//! instance owns the device principal), holds that account's keys and trust
//! store, performs discovery, and exposes a D-Bus interface for desktop
//! integrations and for `lpctl`. See `systemd/ligatured.service` next to this crate.
//!
//! **Status:** scaffold. Starts and exits.

fn main() {
    println!(
        "ligatured {} (scaffold; no protocol logic yet)",
        env!("CARGO_PKG_VERSION")
    );
}
