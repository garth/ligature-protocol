//! `lpctl`: command-line tool for pairing devices and inspecting state.
//!
//! Talks to `ligatured` over D-Bus. Planned subcommands: `status`, `pair`,
//! `devices`, `revoke`, `caps`.
//!
//! **Status:** scaffold. Prints its version and exits.

fn main() {
    println!(
        "lpctl {} (scaffold; no commands yet)",
        env!("CARGO_PKG_VERSION")
    );
}
