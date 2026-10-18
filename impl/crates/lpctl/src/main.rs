//! `lpctl`: command-line tool for pairing devices and inspecting state.
//!
//! Talks to `ligatured` over its control socket. Planned subcommands: `init`,
//! `status`, `pair`, `devices`, `revoke`, `caps`.
//!
//! **Status:** scaffold. Parses its arguments, prints its version and exits.

use clap::{Parser, Subcommand};

/// Command-line options.
#[derive(Debug, Parser)]
#[command(version, about = "Ligature Protocol command-line tool")]
struct Options {
    #[command(subcommand)]
    command: Option<Command>,
}

/// Subcommands. Each is a stub until its milestone task lands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Show the daemon's status.
    Status,
}

fn main() {
    let options = Options::parse();
    match options.command {
        Some(Command::Status) | None => {
            println!(
                "lpctl {} (scaffold; no commands yet)",
                env!("CARGO_PKG_VERSION")
            );
        }
    }
}
