//! `lpctl`: command-line tool for pairing devices and inspecting state.
//!
//! Talks to `ligatured` over its control socket, found through the same
//! directory options and environment variables as the daemon.

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ligatured::control::{
    Caps, Client, ClipboardText, Devices, Empty, Event, InitResult, Request, types,
};
use ligatured::daemon::Status;
use ligatured::grants::Grants;
use lp_core::paths::{DATA_DIR_ENV, Locations, RUNTIME_DIR_ENV};
use lp_core::{Error, PublicKey};

/// Command-line options.
#[derive(Debug, Parser)]
#[command(version, about = "Ligature Protocol command-line tool")]
struct Options {
    /// The daemon's data directory (default: data/).
    #[arg(long, env = DATA_DIR_ENV, global = true)]
    data_dir: Option<PathBuf>,
    /// The daemon's runtime directory (default: the data directory).
    #[arg(long, env = RUNTIME_DIR_ENV, global = true)]
    runtime_dir: Option<PathBuf>,
    /// Answer yes to the code comparison without asking (for scripts).
    #[arg(long, global = true)]
    yes: bool,
    #[command(subcommand)]
    command: Command,
}

/// Subcommands.
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
    /// List this person's devices.
    Devices,
    /// Pair: with no offer, this device is the newcomer and shows an offer;
    /// with an offer, this device introduces the newcomer that showed it.
    Pair {
        /// The offer text shown by the newcomer (`ligature:pair/1?…`).
        offer: Option<String>,
        /// Do not make the newcomer an introducer.
        #[arg(long)]
        no_introducer: bool,
    },
    /// Revoke a device or person key.
    Revoke {
        /// The key (`z6Mk…`).
        key: PublicKey,
    },
    /// Show the live capabilities of connected peers.
    Caps,
    /// List the grants this device has issued.
    Grants,
    /// Read or set the local clipboard through the daemon (diagnostics).
    Clipboard {
        #[command(subcommand)]
        action: ClipboardAction,
    },
    /// Post a test notification from this device to every device that displays them.
    Notify {
        /// The title.
        title: String,
        /// The body.
        body: Option<String>,
    },
}

/// `lpctl clipboard` actions.
#[derive(Debug, Subcommand)]
enum ClipboardAction {
    /// Print the clipboard text.
    Get,
    /// Put text on the clipboard.
    Set {
        /// The text.
        text: String,
    },
}

#[tokio::main]
async fn main() {
    let options = Options::parse();
    let locations = Locations::resolve(options.data_dir.as_deref(), options.runtime_dir.as_deref());
    if let Err(e) = run(&options, &locations).await {
        eprintln!("lpctl: {e}");
        std::process::exit(1);
    }
}

async fn run(options: &Options, locations: &Locations) -> Result<(), Error> {
    let mut client = Client::connect(&locations.control_socket()).await?;
    match &options.command {
        Command::Init { name, device_name } => {
            let mut req = Request::new(types::INIT, 0);
            req.name = Some(name.clone());
            req.device_name = device_name.clone();
            let created: InitResult = client.request(req).await?;
            println!("created person {} ({})", name, created.person);
        }
        Command::Status => {
            let s: Status = client.request(Request::new(types::STATUS, 0)).await?;
            println!("ligatured {}", s.version);
            println!("initialised: {}", s.initialised);
            if let Some(p) = s.person {
                println!("person:      {} ({})", s.name.unwrap_or_default(), p);
            }
            println!(
                "device:      {} ({}…)",
                s.device_name,
                s.binding.fingerprint()
            );
            println!("binding:     {}", s.binding);
            println!("principal:   {}", s.principal);
            println!("port:        {}", s.port);
            println!("peers:       {}", s.peers.len());
            if !s.peers.is_empty() {
                let known: Devices = client.request(Request::new(types::DEVICES, 1)).await?;
                for p in s.peers {
                    let entry = known.devices.iter().find(|d| d.key == p);
                    let name = entry
                        .and_then(|d| d.display_name.clone())
                        .unwrap_or_else(|| "?".to_owned());
                    let platform = entry.and_then(|d| d.platform.clone()).unwrap_or_default();
                    println!("  {:<20} {:<8} {}…  {}", name, platform, p.fingerprint(), p);
                }
            }
        }
        Command::Devices => {
            let d: Devices = client.request(Request::new(types::DEVICES, 0)).await?;
            if d.devices.is_empty() {
                println!("no devices");
            }
            for group in ligatured::control::group_devices(&d.devices) {
                let mut flags = Vec::new();
                if group.this_device() {
                    flags.push("this device");
                }
                if group.introducer() {
                    flags.push("introducer");
                }
                if group.connected() {
                    flags.push("connected");
                }
                if group.revoked() {
                    flags.push("REVOKED");
                }
                let flags = if flags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", flags.join(", "))
                };
                println!(
                    "{:<20} {:<8}{}",
                    group.display_name.clone().unwrap_or_else(|| "?".to_owned()),
                    group.platform.clone().unwrap_or_default(),
                    flags,
                );
                for key in group.keys() {
                    println!(
                        "  {:<9} {}…  {}{}",
                        key.role,
                        key.key.fingerprint(),
                        key.key,
                        if key.revoked { "  REVOKED" } else { "" },
                    );
                }
            }
        }
        Command::Pair {
            offer,
            no_introducer,
        } => {
            let yes = options.yes;
            let req = match offer {
                Some(text) => {
                    let mut r = Request::new(types::PAIR_INTRODUCE, 0);
                    r.offer = Some(text.trim().to_owned());
                    r.introducer = Some(!no_introducer);
                    r
                }
                None => Request::new(types::PAIR_JOIN, 0),
            };
            let response = client
                .call(req, |event: &Event| on_pair_event(event, yes))
                .await?;
            match response.error {
                None => println!("paired."),
                Some(e) => return Err(Error::Chain(format!("{}: {}", e.code, e.message))),
            }
        }
        Command::Revoke { key } => {
            let mut req = Request::new(types::REVOKE, 0);
            req.key = Some(*key);
            let _: Empty = client.request(req).await?;
            println!("revoked {key}");
        }
        Command::Grants => {
            let g: Grants = client.request(Request::new(types::GRANTS, 0)).await?;
            if g.grants.is_empty() {
                println!("no grants");
            }
            for grant in g.grants {
                println!(
                    "{:<9} {:<8} {}  to {} ({}…)  {}",
                    format!("{:?}", grant.basis).to_lowercase(),
                    grant.grantor.fingerprint(),
                    grant.capability,
                    grant.grantee,
                    grant.grantee.fingerprint(),
                    grant.created_at
                );
            }
        }
        Command::Clipboard { action } => match action {
            ClipboardAction::Get => {
                let c: ClipboardText = client
                    .request(Request::new(types::CLIPBOARD_GET, 0))
                    .await?;
                if !c.available {
                    eprintln!("(clipboard unavailable)");
                }
                if let Some(t) = c.text {
                    println!("{t}");
                }
            }
            ClipboardAction::Set { text } => {
                let mut req = Request::new(types::CLIPBOARD_SET, 0);
                req.text = Some(text.clone());
                let _: Empty = client.request(req).await?;
            }
        },
        Command::Notify { title, body } => {
            let mut req = Request::new(types::NOTIFY, 0);
            req.title = Some(title.clone());
            req.body = body.clone();
            let _: Empty = client.request(req).await?;
            println!("posted.");
        }
        Command::Caps => {
            let caps: Caps = client.request(Request::new(types::CAPS, 0)).await?;
            println!("this device offers:");
            if caps.local.is_empty() {
                println!("  (nothing)");
            }
            for d in &caps.local {
                println!(
                    "  {} ({:?}){}",
                    d.capability_type.as_str(),
                    d.scope,
                    directions(d)
                );
            }
            if caps.peers.is_empty() {
                println!("no connected peers");
            }
            for p in caps.peers {
                println!("{} ({}…) offers:", p.peer, p.peer.fingerprint());
                if p.capabilities.is_empty() {
                    println!("  (nothing)");
                }
                for d in &p.capabilities {
                    println!(
                        "  {} ({:?}){}",
                        d.capability_type.as_str(),
                        d.scope,
                        directions(d)
                    );
                }
            }
        }
    }
    Ok(())
}

/// Shows the offer or the code; answers the code with a confirmation.
fn on_pair_event(event: &Event, yes: bool) -> Option<Request> {
    match event.event_type.as_str() {
        types::PAIR_OFFER => {
            let text = event.offer.clone().unwrap_or_default();
            match qrcode::QrCode::new(text.as_bytes()) {
                Ok(code) => {
                    let image = code
                        .render::<qrcode::render::unicode::Dense1x2>()
                        .quiet_zone(true)
                        .build();
                    println!("{image}");
                }
                Err(e) => eprintln!("(cannot render QR code: {e})"),
            }
            println!("Scan the code above or run on an introducer:\n  lpctl pair '{text}'\n");
            println!("Waiting for an introducer…");
            None
        }
        types::PAIR_CODE => {
            let code = event.code.clone().unwrap_or_default();
            let split = 3.min(code.len());
            println!("\n  Code: {}  {}\n", &code[..split], &code[split..]);
            let ok = yes || ask("Does the other device show the same code? [y/N] ");
            let mut r = Request::new(
                if ok {
                    types::PAIR_CONFIRM
                } else {
                    types::PAIR_ABORT
                },
                0,
            );
            r.pairing = Some(event.id);
            Some(r)
        }
        _ => None,
    }
}

fn ask(question: &str) -> bool {
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    matches!(line.trim(), "y" | "Y" | "yes" | "YES")
}

/// The directions a notifications descriptor declares, for `caps`.
fn directions(d: &lp_core::capability::Descriptor) -> String {
    match lp_core::capability::NotificationsParams::from_descriptor(d) {
        Some(p) => format!(
            "  emits={} displays={} maxIconBytes={}",
            p.emits, p.displays, p.max_icon_bytes
        ),
        None => String::new(),
    }
}
