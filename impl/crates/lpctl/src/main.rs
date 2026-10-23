//! `lpctl`: command-line tool for pairing devices and inspecting state.
//!
//! Talks to `ligatured` over its control socket, found through the same
//! directory options and environment variables as the daemon.

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use std::sync::{Arc, Mutex};

use ligatured::control::{
    Caps, Client, ClipboardText, Devices, Empty, Event, InitResult, Offers, Request, Transfers,
    types,
};
use ligatured::daemon::Status;
use ligatured::files_sync::{FileState, TransferEnd, TransferProgress};
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
    /// Send files to one of this person's devices, fetch offered ones, or
    /// manage what this device offers.
    Files {
        #[command(subcommand)]
        action: FilesAction,
    },
}

/// `lpctl files` actions.
#[derive(Debug, Subcommand)]
enum FilesAction {
    /// Push files to a device; they land in its download folder.
    Send {
        /// The device: its name, or a prefix of its key or fingerprint.
        device: String,
        /// The files.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Fetch files a device offered, by id, into the download folder.
    Get {
        /// The device: its name, or a prefix of its key or fingerprint.
        device: String,
        /// The ids the device offered.
        #[arg(required = true)]
        ids: Vec<String>,
        /// A directory or file path to land in instead.
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// Offer a file for other devices to fetch; prints its id.
    Offer {
        /// The file.
        path: PathBuf,
    },
    /// List what this device offers.
    Offers,
    /// Withdraw an offer.
    Withdraw {
        /// The id.
        id: String,
    },
    /// Cancel a transfer in flight.
    Cancel {
        /// The transfer id.
        transfer: String,
    },
    /// List the transfers in flight.
    Transfers,
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
                for p in &c.parts {
                    eprintln!(
                        "({} {}{})",
                        p.mime,
                        human(p.size),
                        if p.promised { ", promised" } else { "" }
                    );
                }
                for f in &c.files {
                    eprintln!("(file {f}, promised)");
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
        Command::Files { action } => files(&mut client, locations, action).await?,
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

/// `lpctl files`.
async fn files(
    client: &mut Client,
    locations: &Locations,
    action: &FilesAction,
) -> Result<(), Error> {
    match action {
        FilesAction::Send { device, paths } => {
            let peer = resolve_device(client, device).await?;
            let mut req = Request::new(types::FILES_SEND, 0);
            req.key = Some(peer);
            req.paths = Some(paths.iter().map(absolute).collect::<Result<_, _>>()?);
            run_transfer(client, locations, req).await
        }
        FilesAction::Get { device, ids, to } => {
            let peer = resolve_device(client, device).await?;
            let mut req = Request::new(types::FILES_GET, 0);
            req.key = Some(peer);
            req.ids = Some(ids.clone());
            req.to = to.as_ref().map(absolute).transpose()?;
            run_transfer(client, locations, req).await
        }
        FilesAction::Offer { path } => {
            let mut req = Request::new(types::FILES_OFFER, 0);
            req.paths = Some(vec![absolute(path)?]);
            let file: lp_core::files::FileDescriptor = client.request(req).await?;
            println!("{}", file.id);
            eprintln!(
                "offered {} ({}, {}) until withdrawn or the daemon stops",
                file.name,
                human(file.size),
                file.mime
            );
            Ok(())
        }
        FilesAction::Offers => {
            let o: Offers = client.request(Request::new(types::FILES_OFFERS, 0)).await?;
            if o.offers.is_empty() {
                println!("no offers");
            }
            for f in o.offers {
                println!("{}  {:<40} {:>10}  {}", f.id, f.name, human(f.size), f.mime);
            }
            println!("received files land in {}", o.location);
            Ok(())
        }
        FilesAction::Withdraw { id } => {
            let mut req = Request::new(types::FILES_WITHDRAW, 0);
            req.ids = Some(vec![id.clone()]);
            let _: Empty = client.request(req).await?;
            println!("withdrawn {id}");
            Ok(())
        }
        FilesAction::Cancel { transfer } => {
            let mut req = Request::new(types::FILES_CANCEL, 0);
            req.transfer = Some(transfer.clone());
            let _: Empty = client.request(req).await?;
            println!("cancelled {transfer}");
            Ok(())
        }
        FilesAction::Transfers => {
            let t: Transfers = client
                .request(Request::new(types::FILES_TRANSFERS, 0))
                .await?;
            if t.transfers.is_empty() {
                println!("no transfers in flight");
            }
            for p in t.transfers {
                println!(
                    "{}  {}  {}  {}/{}",
                    p.transfer,
                    if p.outgoing { "to  " } else { "from" },
                    p.peer.fingerprint(),
                    human(p.done_bytes()),
                    human(p.total_bytes())
                );
                for f in p.files {
                    println!(
                        "  {:<40} {:>10}  {}",
                        f.name,
                        human(f.size),
                        state(&f.state)
                    );
                }
            }
            Ok(())
        }
    }
}

/// Runs a send or a get: progress on standard error, Ctrl-C cancels, the
/// outcome decides the exit status.
async fn run_transfer(
    client: &mut Client,
    locations: &Locations,
    req: Request,
) -> Result<(), Error> {
    let transfer_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let for_signal = transfer_id.clone();
    let socket = locations.control_socket();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        let id = for_signal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        eprintln!(
            "
cancelling…"
        );
        if let Some(id) = id
            && let Ok(mut second) = Client::connect(&socket).await
        {
            let mut cancel = Request::new(types::FILES_CANCEL, 0);
            cancel.transfer = Some(id);
            let _ = second.request::<Empty>(cancel).await;
        } else {
            std::process::exit(130);
        }
    });
    let mut last_line = String::new();
    let response = client
        .call(req, |event: &Event| {
            if let Some(p) = &event.progress {
                *transfer_id
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(p.transfer.clone());
                let line = progress_line(p);
                if line != last_line {
                    eprint!(
                        "
[K{line}"
                    );
                    last_line = line;
                }
            }
            None
        })
        .await?;
    if !last_line.is_empty() {
        eprintln!();
    }
    let outcome: TransferProgress = response.into_ok()?;
    for f in &outcome.files {
        println!("{:<40} {:>10}  {}", f.name, human(f.size), state(&f.state));
    }
    match outcome.finished {
        Some(TransferEnd::Complete) if outcome.all_received() => {
            println!(
                "done: {} in {} file(s)",
                human(outcome.done_bytes()),
                outcome.files.len()
            );
            Ok(())
        }
        Some(TransferEnd::Complete) => Err(Error::Io("some files were not received".to_owned())),
        Some(TransferEnd::Declined { reason }) => Err(Error::Io(format!("declined: {reason}"))),
        Some(TransferEnd::Cancelled) => Err(Error::Io("cancelled".to_owned())),
        Some(TransferEnd::Failed { message }) => Err(Error::Io(format!("failed: {message}"))),
        None => Err(Error::Io(
            "the daemon answered before the transfer ended".to_owned(),
        )),
    }
}

fn progress_line(p: &TransferProgress) -> String {
    let total = p.total_bytes();
    let done = p.done_bytes();
    let percent = done.saturating_mul(100).checked_div(total).unwrap_or(100);
    let active = p
        .files
        .iter()
        .filter(|f| f.state == FileState::Active)
        .map(|f| f.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let finished = p
        .files
        .iter()
        .filter(|f| !matches!(f.state, FileState::Pending | FileState::Active))
        .count();
    format!(
        "{percent:>3}%  {}/{}  {finished}/{} files  {active}",
        human(done),
        human(total),
        p.files.len()
    )
}

fn state(s: &FileState) -> String {
    match s {
        FileState::Pending => "pending".to_owned(),
        FileState::Active => "active".to_owned(),
        FileState::Received { location: Some(l) } => format!("received  {l}"),
        FileState::Received { location: None } => "received".to_owned(),
        FileState::Failed {
            reason: Some(r), ..
        } => format!("FAILED  {r}"),
        FileState::Failed { message, .. } => format!("FAILED  {message}"),
        FileState::Cancelled => "cancelled".to_owned(),
    }
}

/// Bytes as a short human figure.
fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The daemon runs elsewhere, so paths cross the socket absolute.
fn absolute(path: &PathBuf) -> Result<String, Error> {
    std::path::absolute(path)
        .map(|p| p.display().to_string())
        .map_err(|e| Error::Io(format!("{}: {e}", path.display())))
}

/// A device by its key, or by its name or a prefix of its key or fingerprint
/// among this person's bindings.
async fn resolve_device(client: &mut Client, device: &str) -> Result<PublicKey, Error> {
    if let Ok(key) = PublicKey::parse(device) {
        return Ok(key);
    }
    let known: Devices = client.request(Request::new(types::DEVICES, 0)).await?;
    let needle = device.to_lowercase();
    let matches: Vec<_> = known
        .devices
        .iter()
        .filter(|d| d.role == "binding" && !d.this_device && !d.revoked)
        .filter(|d| {
            d.display_name
                .as_deref()
                .is_some_and(|n| n.to_lowercase() == needle)
                || d.key.to_string().starts_with(device)
                || d.key.fingerprint().starts_with(device)
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.key),
        [] => Err(Error::Decode(format!(
            "no device named {device}; see lpctl devices"
        ))),
        many => Err(Error::Decode(format!(
            "{device} names {} devices; use the key",
            many.len()
        ))),
    }
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

/// The directions a notifications or files descriptor declares, for `caps`.
fn directions(d: &lp_core::capability::Descriptor) -> String {
    if let Some(p) = lp_core::capability::NotificationsParams::from_descriptor(d) {
        return format!(
            "  emits={} displays={} maxIconBytes={}",
            p.emits, p.displays, p.max_icon_bytes
        );
    }
    if let Some(p) = lp_core::files::FilesParams::from_descriptor(d) {
        return format!(
            "  send={} receive={} maxChunkBytes={} maxStreams={}",
            p.send, p.receive, p.max_chunk_bytes, p.max_streams
        );
    }
    if let Some(p) = lp_core::capability::ClipboardParams::from_descriptor(d) {
        return format!(
            "  maxBytes={} mimeTypes={} fetch={}",
            p.max_bytes,
            p.mime_types.join(","),
            p.fetch
        );
    }
    String::new()
}
