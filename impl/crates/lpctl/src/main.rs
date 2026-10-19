//! `lpctl`: command-line tool for pairing devices and inspecting state.
//!
//! Talks to `ligatured` over its control socket, found through the same
//! directory options and environment variables as the daemon. Planned
//! subcommands: `init`, `status`, `pair`, `devices`, `revoke`, `caps`.
//!
//! **Status:** scaffold. Parses its arguments, prints its version and exits.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use lp_core::paths::{DATA_DIR_ENV, Locations, RUNTIME_DIR_ENV};

/// Command-line options.
#[derive(Debug, Parser)]
#[command(version, about = "Ligature Protocol command-line tool")]
struct Options {
    /// The daemon's data directory (default: data/).
    #[arg(long, env = DATA_DIR_ENV)]
    data_dir: Option<PathBuf>,
    /// The daemon's runtime directory (default: the data directory).
    #[arg(long, env = RUNTIME_DIR_ENV)]
    runtime_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Subcommands. Each is a stub until its milestone task lands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Create this person's keys and this device's keys on a fresh daemon.
    Init {
        /// The person's display name, self-asserted (ADR 0032).
        #[arg(long)]
        name: String,
        /// This device's display name (default: the host name, ADR 0034).
        #[arg(long)]
        device_name: Option<String>,
    },
    /// Show the daemon's status.
    Status,
}

fn main() {
    let options = Options::parse();
    let locations = Locations::resolve(options.data_dir.as_deref(), options.runtime_dir.as_deref());
    match options.command {
        Some(Command::Init { name, device_name }) => {
            println!(
                "lpctl init: would create keys for {name:?} on device {} via {} (scaffold)",
                device_name.unwrap_or_else(|| "<host name>".to_owned()),
                locations.control_socket().display()
            );
        }
        Some(Command::Status) | None => {
            println!(
                "lpctl {} (scaffold; no commands yet; socket {})",
                env!("CARGO_PKG_VERSION"),
                locations.control_socket().display()
            );
        }
    }
}
