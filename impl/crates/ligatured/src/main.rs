//! `ligatured`: the Ligature Protocol user daemon.
//!
//! Runs as a user service for one account's binding (a system-level instance owns
//! the device principal), holds that account's keys and trust store, performs
//! discovery, and exposes a control socket for `lpctl` and desktop integrations.
//! See `systemd/ligatured.service` next to this crate.
//!
//! Logging goes to stderr through `tracing`; systemd and launchd capture it. The
//! level is set with `RUST_LOG` and defaults to `info`.
//!
//! **Status:** scaffold. Starts, logs a line and exits.

use clap::Parser;
use tracing_subscriber::EnvFilter;

/// Command-line options of the daemon.
#[derive(Debug, Parser)]
#[command(version, about = "Ligature Protocol user daemon")]
struct Options {}

fn main() {
    let _options = Options::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "ligatured starting (scaffold; no protocol logic yet)"
    );
}
