//! `ligatured`: the Ligature Protocol user daemon.
//!
//! Runs as a user service for one account's binding (a system-level instance
//! will own the device principal later), holds that account's keys and trust
//! store, performs discovery, and exposes a control socket for `lpctl` and
//! desktop integrations. See `systemd/ligatured.service` next to this crate.
//!
//! There is no configuration file. The daemon takes its data and runtime
//! directories from the command line or the environment (`LIGATURE_DATA_DIR`,
//! `LIGATURE_RUNTIME_DIR`), defaulting to `data/` under the working directory for
//! milestone 1. Logging goes to stderr through `tracing`; systemd and launchd
//! capture it. The level is set with `RUST_LOG` and defaults to `info`.

use std::path::PathBuf;

use clap::Parser;
use lp_core::paths::{DATA_DIR_ENV, Locations, RUNTIME_DIR_ENV};
use tracing_subscriber::EnvFilter;

/// Command-line options of the daemon.
#[derive(Debug, Parser)]
#[command(version, about = "Ligature Protocol user daemon")]
struct Options {
    /// Directory holding the store and the key file (default: data/).
    #[arg(long, env = DATA_DIR_ENV)]
    data_dir: Option<PathBuf>,
    /// Directory holding the control socket (default: the data directory).
    #[arg(long, env = RUNTIME_DIR_ENV)]
    runtime_dir: Option<PathBuf>,
    /// This device's display name on first start (default: the host name).
    #[arg(long)]
    device_name: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Internal subcommands.
#[derive(Debug, clap::Subcommand)]
enum Command {
    /// The Wayland clipboard helper the daemon starts for itself (Linux).
    #[command(hide = true)]
    ClipboardHelper,
}

#[tokio::main]
async fn main() {
    let options = Options::parse();
    if let Some(Command::ClipboardHelper) = options.command {
        #[cfg(target_os = "linux")]
        {
            if let Err(e) = ligatured::clipboard_linux::run_helper() {
                eprintln!("clipboard helper: {e}");
                std::process::exit(1);
            }
            return;
        }
        #[cfg(not(target_os = "linux"))]
        {
            eprintln!("clipboard helper is Linux only");
            std::process::exit(1);
        }
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let locations = Locations::resolve(options.data_dir.as_deref(), options.runtime_dir.as_deref());
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        data_dir = %locations.data_dir.display(),
        control_socket = %locations.control_socket().display(),
        "ligatured starting"
    );
    if let Err(e) = ligatured::run::run(locations, options.device_name.as_deref()).await {
        tracing::error!(%e, "ligatured stopped");
        std::process::exit(1);
    }
}
