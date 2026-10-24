//! `ligatured`: the Ligature Protocol user daemon.
//!
//! Runs as a user service for one account's binding (a system-level instance
//! will own the device principal later), holds that account's keys and trust
//! store, performs discovery, and exposes a control socket for `lpctl` and
//! desktop integrations. See `systemd/ligatured.service` next to this crate.
//!
//! There is no configuration file. The daemon takes its data and runtime
//! directories from the command line or the environment (`LIGATURE_DATA_DIR`,
//! `LIGATURE_RUNTIME_DIR`), defaulting to the platform locations of
//! `lp_core::paths`. Logging goes to stderr through `tracing`; systemd and launchd
//! capture it. The level is set with `RUST_LOG` and defaults to `info`.

use std::path::PathBuf;

use clap::Parser;
use ligatured::run::PORT_ENV;
use lp_core::paths::{DATA_DIR_ENV, Locations, RUNTIME_DIR_ENV};
use tracing_subscriber::EnvFilter;

/// Command-line options of the daemon.
#[derive(Debug, Parser)]
#[command(version, about = "Ligature Protocol user daemon")]
struct Options {
    /// Directory holding the store and the key file (default: the platform's
    /// data directory, ~/.local/share/ligature or ~/Library/Application
    /// Support/ligature).
    #[arg(long, env = DATA_DIR_ENV)]
    data_dir: Option<PathBuf>,
    /// Directory holding the control socket (default: the platform's runtime
    /// directory, $XDG_RUNTIME_DIR/ligature or $TMPDIR/ligature, or the data
    /// directory when that is given).
    #[arg(long, env = RUNTIME_DIR_ENV)]
    runtime_dir: Option<PathBuf>,
    /// This device's display name on first start (default: the host name).
    #[arg(long)]
    device_name: Option<String>,
    /// UDP port for QUIC (default: one the operating system assigns). Fix it
    /// where a firewall must allow peers to dial in.
    #[arg(long, env = PORT_ENV)]
    port: Option<u16>,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Internal subcommands.
#[derive(Debug, clap::Subcommand)]
// The variants are the helper subcommands the daemon spawns by name.
#[allow(clippy::enum_variant_names)]
enum Command {
    /// The Wayland clipboard helper the daemon starts for itself (Linux).
    #[command(hide = true)]
    ClipboardHelper,
    /// The Wayland input helper the daemon starts for itself (Linux).
    #[command(hide = true)]
    InputHelper,
    /// The Wayland screen helper the daemon starts for itself (Linux):
    /// `list`, or `capture OUTPUT RATE`.
    #[command(hide = true)]
    ScreenHelper {
        /// The command and its arguments.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() {
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
    if let Some(Command::InputHelper) = options.command {
        #[cfg(target_os = "linux")]
        {
            if let Err(e) = ligatured::input_linux::run_helper() {
                eprintln!("input helper: {e}");
                std::process::exit(1);
            }
            return;
        }
        #[cfg(not(target_os = "linux"))]
        {
            eprintln!("input helper is Linux only");
            std::process::exit(1);
        }
    }
    if let Some(Command::ScreenHelper { args }) = &options.command {
        #[cfg(target_os = "linux")]
        {
            if let Err(e) = ligatured::screen_linux::run_helper(args) {
                eprintln!("screen helper: {e}");
                std::process::exit(1);
            }
            return;
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = args;
            eprintln!("screen helper is Linux only");
            std::process::exit(1);
        }
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    // On macOS the pasteboard answers a paste of a promised type through
    // the main thread's run loop, so the daemon's work runs on another
    // thread and the main thread runs that loop.
    #[cfg(target_os = "macos")]
    {
        std::thread::Builder::new()
            .name("ligatured".into())
            .spawn(move || {
                runtime.block_on(daemon(options));
                std::process::exit(0);
            })
            .expect("spawn the daemon thread");
        lp_macos::run_main_loop();
    }
    #[cfg(not(target_os = "macos"))]
    {
        runtime.block_on(daemon(options));
    }
}

/// The log filter: `RUST_LOG`, or `info` when it is unset or empty, with the
/// portal proxies' noise quieted. The portal wrapper's proxies have no
/// properties to cache, which zbus reports at warn level on every request
/// object; the service sets `RUST_LOG=info`, so the quieting applies whatever
/// it says, unless it names zbus itself, for someone debugging the bus.
fn log_directives(rust_log: Option<&str>) -> String {
    let base = rust_log
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("info");
    if base.contains("zbus") {
        base.to_owned()
    } else {
        format!("{base},zbus::proxy=error")
    }
}

/// The daemon proper, once the runtime is up.
async fn daemon(options: Options) {
    let directives = log_directives(std::env::var("RUST_LOG").ok().as_deref());
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_new(&directives)
                .unwrap_or_else(|_| EnvFilter::new(log_directives(None))),
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
    if let Err(e) =
        ligatured::run::run(locations, options.device_name.as_deref(), options.port).await
    {
        tracing::error!(%e, "ligatured stopped");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::log_directives;

    #[test]
    fn the_portal_proxies_are_quieted_whatever_rust_log_says() {
        assert_eq!(log_directives(None), "info,zbus::proxy=error");
        assert_eq!(log_directives(Some("")), "info,zbus::proxy=error");
        assert_eq!(log_directives(Some("info")), "info,zbus::proxy=error");
        assert_eq!(
            log_directives(Some("debug,quinn=warn")),
            "debug,quinn=warn,zbus::proxy=error"
        );
        // Someone debugging the bus gets what they asked for.
        assert_eq!(log_directives(Some("info,zbus=debug")), "info,zbus=debug");
    }
}
