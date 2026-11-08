//! `lpctl`: command-line tool for pairing devices and inspecting state.
//!
//! Talks to `ligatured` over its control socket, found through the same
//! directory options and environment variables as the daemon.

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use std::sync::{Arc, Mutex};

use ligatured::control::{
    Caps, Client, ClipboardPasted, ClipboardText, Devices, Empty, Event, InitResult, InputLayout,
    Offers, Permissions, Request, ScreenDisplays, ScreenStopped, Transfers, types,
};
use ligatured::daemon::Status;
use ligatured::files_sync::{FileState, TransferEnd, TransferProgress};
use ligatured::grants::Grants;
use ligatured::input::InputStatus;
use ligatured::permissions::Permission;
use ligatured::screen::ScreenStatus;
use lp_core::paths::{DATA_DIR_ENV, Locations, RUNTIME_DIR_ENV};
use lp_core::{Error, PublicKey};

mod upgrade;

/// Command-line options.
#[derive(Debug, Parser)]
#[command(version, about = "Ligature Protocol command-line tool")]
struct Options {
    /// The daemon's data directory (default: the platform's, ~/.local/share/ligature
    /// or ~/Library/Application Support/ligature).
    #[arg(long, env = DATA_DIR_ENV, global = true)]
    data_dir: Option<PathBuf>,
    /// The daemon's runtime directory, holding the control socket (default: the
    /// platform's, $XDG_RUNTIME_DIR/ligature or $TMPDIR/ligature, or the data
    /// directory when that is given).
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
    /// Install the latest release on this device: verified against the project's
    /// release key, then its install.sh replaces the binaries and restarts the service.
    Upgrade {
        /// Only say whether a newer release exists.
        #[arg(long)]
        check: bool,
    },
    /// The detached half of the daily check for a new release: refreshes the
    /// check file in the data directory.
    #[command(hide = true)]
    RefreshUpdateCache,
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
    /// One keyboard and mouse across this person's desktops: which edge of
    /// this screen leads to which device, and the drive in progress.
    Input {
        #[command(subcommand)]
        action: InputAction,
    },
    /// Watch a device's screen or manage the shares this device runs
    /// (diagnostics: the encoded stream goes to a file, no window).
    Screen {
        #[command(subcommand)]
        action: ScreenAction,
    },
    /// The permissions macOS asks for: which the daemon holds, or reset them
    /// so macOS asks again, or open one's page in System Settings.
    Permissions {
        #[command(subcommand)]
        action: Option<PermissionsAction>,
    },
}

/// `lpctl permissions` actions (macOS).
#[derive(Debug, Subcommand)]
enum PermissionsAction {
    /// Forget Ligature's Accessibility, Input Monitoring and Screen Recording
    /// permissions and restart the daemon, so macOS asks for them again.
    Reset,
    /// Open a permission's page in System Settings, where it is turned on.
    Open {
        /// accessibility, input-monitoring or screen-recording.
        permission: String,
    },
}

/// `lpctl screen` actions.
#[derive(Debug, Subcommand)]
enum ScreenAction {
    /// List a device's displays.
    Displays {
        /// The device: its name, or a prefix of its key or fingerprint.
        device: String,
    },
    /// Watch a device's display: the encoded H.264 stream is written to a file
    /// (play it with `ffplay -f h264 FILE`), progress to the terminal.
    Start {
        /// The device: its name, or a prefix of its key or fingerprint.
        device: String,
        /// The display id, from `displays`; the primary one by default.
        #[arg(long)]
        display: Option<String>,
        /// The widest picture wanted.
        #[arg(long, default_value_t = 1280)]
        width: u32,
        /// The tallest picture wanted.
        #[arg(long, default_value_t = 720)]
        height: u32,
        /// The frame rate wanted.
        #[arg(long, default_value_t = 30)]
        rate: u32,
        /// Where the stream goes.
        #[arg(long)]
        out: PathBuf,
        /// Stop after this many frames.
        #[arg(long)]
        frames: Option<u64>,
    },
    /// Stop the shares with a device, or every share this device runs.
    Stop {
        /// The device: its name, or a prefix of its key or fingerprint.
        device: Option<String>,
    },
    /// What this device shares and watches.
    Status,
}

/// `lpctl input` actions.
#[derive(Debug, Subcommand)]
enum InputAction {
    /// Show which edges lead where, or change one.
    Layout {
        #[command(subcommand)]
        action: Option<LayoutAction>,
    },
    /// What this device advertises, drives and is driven by, with the round trip.
    Status,
    /// Give the input back: end the drive this device runs, as pressing both
    /// Control keys or both Alt (Option) keys together does.
    Release,
}

/// `lpctl input layout` actions.
#[derive(Debug, Subcommand)]
enum LayoutAction {
    /// Make an edge of this screen lead to a device.
    Set {
        /// left, right, top or bottom.
        edge: String,
        /// The device: its name, or a prefix of its key or fingerprint.
        device: String,
    },
    /// Make an edge lead nowhere.
    Unset {
        /// left, right, top or bottom.
        edge: String,
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
    /// Put one typed part, read from a file, on the clipboard (diagnostics).
    Put {
        /// The content type, such as image/png.
        mime: String,
        /// The file whose bytes form the part.
        path: PathBuf,
    },
    /// Paste one type from the clipboard into a file, fetching it if
    /// promised (diagnostics).
    Paste {
        /// The content type.
        mime: String,
        /// Where the daemon writes the bytes.
        out: PathBuf,
    },
}

#[tokio::main]
async fn main() {
    let options = Options::parse();
    let locations = Locations::resolve(options.data_dir.as_deref(), options.runtime_dir.as_deref());
    let current = upgrade::current_version();
    let mut newer = None;
    let result = match &options.command {
        Command::Upgrade { check } => upgrade_command(*check, current.clone()),
        Command::RefreshUpdateCache => {
            upgrade::refresh(&upgrade::Source::default(), &locations, &current)
        }
        Command::Permissions {
            action: Some(PermissionsAction::Reset),
        } => permissions_reset(),
        Command::Permissions {
            action: Some(PermissionsAction::Open { permission }),
        } => permissions_open(permission),
        _ => {
            newer = upgrade::begin(&locations, &current);
            run(&options, &locations).await
        }
    };
    let failed = result.is_err();
    if let Err(e) = result {
        eprintln!("lpctl: {e}");
    }
    // The notice comes last, after the command's own output or its error.
    if let Some(newer) = newer {
        eprintln!("{}", upgrade::notice(&newer, &current));
    }
    if failed {
        std::process::exit(1);
    }
}

/// The bundle identifier macOS grants Ligature's permissions to, which is
/// also the launchd agent's label.
const MAC_BUNDLE: &str = "org.ligatureprotocol.ligatured";

/// The `tccutil` services behind the three permissions (`PostEvent` is the
/// posting half of Accessibility).
const TCC_SERVICES: [&str; 4] = ["Accessibility", "PostEvent", "ListenEvent", "ScreenCapture"];

/// The System Settings page where a permission is turned on.
fn settings_url(permission: Permission) -> &'static str {
    match permission {
        Permission::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        Permission::InputMonitoring => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
        Permission::ScreenRecording => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
    }
}

fn mac_only() -> Result<(), Error> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(Error::Refused(
            "only macOS asks for these permissions; this device needs none".to_owned(),
        ))
    }
}

/// Runs a system command, failing with its own message.
fn system(program: &str, args: &[&str]) -> Result<String, Error> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::Io(format!("{program}: {e}")))?;
    if !out.status.success() {
        return Err(Error::Io(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// `lpctl permissions reset`: macOS forgets the answers, and the restarted
/// daemon asks for Accessibility and Input Monitoring at once and for Screen
/// Recording at the first share.
fn permissions_reset() -> Result<(), Error> {
    mac_only()?;
    for service in TCC_SERVICES {
        system("tccutil", &["reset", service, MAC_BUNDLE])?;
    }
    println!("reset Accessibility, Input Monitoring and Screen Recording for Ligature");
    let uid = system("id", &["-u"])?;
    system(
        "launchctl",
        &["kickstart", "-k", &format!("gui/{uid}/{MAC_BUNDLE}")],
    )?;
    println!(
        "restarted the daemon: macOS asks for Accessibility and Input Monitoring now, and for \
         Screen Recording when a device first watches this screen. Once each is allowed the \
         daemon restarts by itself to use it; lpctl permissions shows where things stand."
    );
    Ok(())
}

/// `lpctl permissions` without an action: each permission with whether the
/// daemon holds it and what it enables, then the `open` command for each
/// missing one and `reset`, so the output itself says how the command is used.
fn permissions_report(held: &Permissions) -> String {
    use std::fmt::Write as _;
    if held.permissions.is_empty() {
        return "this device needs no permissions\n".to_owned();
    }
    let mut out = String::new();
    for p in &held.permissions {
        let _ = writeln!(
            out,
            "{:<17} {:<8} {}",
            p.label,
            if p.granted { "granted" } else { "missing" },
            p.enables
        );
    }
    let missing: Vec<_> = held.permissions.iter().filter(|p| !p.granted).collect();
    if missing.is_empty() {
        out.push_str("all granted\n");
    }
    let mut steps: Vec<(String, String)> = missing
        .iter()
        .map(|p| {
            (
                format!("lpctl permissions open {}", p.name),
                format!(
                    "opens the {} page in System Settings, where it is turned on; the daemon \
                     restarts by itself once it is",
                    p.label
                ),
            )
        })
        .collect();
    steps.push((
        "lpctl permissions reset".to_owned(),
        "forgets all three and restarts the daemon, so macOS asks again".to_owned(),
    ));
    let steps: Vec<(&str, &str)> = steps
        .iter()
        .map(|(c, d)| (c.as_str(), d.as_str()))
        .collect();
    out.push_str(&next_steps(&steps));
    out
}

/// A listing's last lines: the commands that act on what it showed, each with
/// what it does, aligned, after a blank line. Every `lpctl` listing ends with
/// one, so the output says how the tool is used (LP-170). Empty for no steps.
fn next_steps(steps: &[(&str, &str)]) -> String {
    use std::fmt::Write as _;
    let Some(width) = steps.iter().map(|(command, _)| command.len()).max() else {
        return String::new();
    };
    let mut out = String::from("\n");
    for (command, does) in steps {
        let _ = writeln!(out, "  {command:<width$}  {does}");
    }
    out
}

const PAIR_ADDS: (&str, &str) = (
    "lpctl pair",
    "adds a device: run it on the new device and give the offer it shows to lpctl pair <OFFER> \
     here",
);

/// After `lpctl status`: how a fresh daemon gets its keys, or what to run next.
fn status_next_steps(initialised: bool) -> String {
    if initialised {
        next_steps(&[
            ("lpctl devices", "lists every device with its keys"),
            PAIR_ADDS,
        ])
    } else {
        next_steps(&[
            (
                "lpctl init --name <NAME>",
                "creates your keys on this, your first device",
            ),
            (
                "lpctl pair",
                "joins your existing devices instead: this device shows an offer, which you give \
                 to lpctl pair <OFFER> on one of them",
            ),
        ])
    }
}

/// After `lpctl devices`: adding one, and revoking one when there is another.
fn devices_next_steps(any_other: bool) -> String {
    let mut steps = vec![PAIR_ADDS];
    if any_other {
        steps.push((
            "lpctl revoke <KEY>",
            "revokes a device's or person's key, on every device",
        ));
    }
    next_steps(&steps)
}

/// After `lpctl input layout`: setting an edge, and unsetting one when any leads somewhere.
fn layout_next_steps(edges: usize) -> String {
    let mut steps = vec![(
        "lpctl input layout set <EDGE> <DEVICE>",
        "makes the left, right, top or bottom edge of this screen lead to a device",
    )];
    if edges > 0 {
        steps.push(("lpctl input layout unset <EDGE>", "makes it lead nowhere"));
    }
    next_steps(&steps)
}

/// After `lpctl input status`: releasing a drive in progress, and the layout.
fn input_status_next_steps(driving: bool) -> String {
    let mut steps = Vec::new();
    if driving {
        steps.push((
            "lpctl input release",
            "gives the input back to this device, as both Control or both Alt keys do",
        ));
    }
    steps.push((
        "lpctl input layout",
        "shows and changes which edge of this screen leads where",
    ));
    next_steps(&steps)
}

/// After `lpctl screen status`: watching a device, and stopping when anything runs.
fn screen_status_next_steps(active: bool) -> String {
    let mut steps = vec![
        (
            "lpview <DEVICE>",
            "watches and drives a device's screen in a window",
        ),
        (
            "lpctl screen displays <DEVICE>",
            "lists a device's displays",
        ),
    ];
    if active {
        steps.push((
            "lpctl screen stop [DEVICE]",
            "stops the shares with a device, or every share this device runs",
        ));
    }
    next_steps(&steps)
}

/// After `lpctl files offers`: offering a file, and withdrawing one when any stands.
fn offers_next_steps(offers: usize) -> String {
    let mut steps = vec![(
        "lpctl files offer <PATH>",
        "offers a file for your other devices to fetch with lpctl files get <DEVICE> <ID>",
    )];
    if offers > 0 {
        steps.push(("lpctl files withdraw <ID>", "withdraws one"));
    }
    next_steps(&steps)
}

/// After `lpctl files transfers`: sending files, and cancelling when any is in flight.
fn transfers_next_steps(in_flight: usize) -> String {
    let mut steps = vec![(
        "lpctl files send <DEVICE> <PATHS>...",
        "pushes files to a device's download folder",
    )];
    if in_flight > 0 {
        steps.push(("lpctl files cancel <TRANSFER>", "cancels one"));
    }
    next_steps(&steps)
}

/// The error `lpctl pair` reports. A join refused because this device already
/// has its keys says how a device is added instead.
fn pair_refusal(join: bool, refused: &ligatured::control::ErrorBody) -> Error {
    if join && refused.code == "already-initialised" {
        return Error::Refused(
            "this device already has its keys. To add a new device, run lpctl pair on it and \
             give the offer it shows to lpctl pair <OFFER> here"
                .to_owned(),
        );
    }
    Error::Refused(refused.message.clone())
}

/// `lpctl permissions open`: the permission's page in System Settings.
fn permissions_open(name: &str) -> Result<(), Error> {
    mac_only()?;
    let permission = Permission::from_name(name).ok_or_else(|| {
        Error::Refused(format!(
            "no permission {name}; use accessibility, input-monitoring or screen-recording"
        ))
    })?;
    system("open", &[settings_url(permission)])?;
    println!(
        "opened {} in System Settings: turn Ligature on there",
        permission.label()
    );
    Ok(())
}

/// `lpctl upgrade`: no daemon needed, so it runs before the control socket is
/// opened, and it may replace this very binary.
fn upgrade_command(check_only: bool, current: semver::Version) -> Result<(), Error> {
    let upgrader = upgrade::Upgrader::new(upgrade::Source::default(), current.clone());
    let mut out = std::io::stdout();
    match upgrader.run(check_only, &mut out)? {
        upgrade::Outcome::UpToDate => println!("{current} is the latest release"),
        upgrade::Outcome::Available(v) => {
            println!(
                "Ligature Protocol {v} is available (this device has {current}); lpctl upgrade installs it"
            );
        }
        upgrade::Outcome::Installed(v) => {
            println!(
                "installed {v}; update your other devices too, and lpctl status shows the daemon"
            );
        }
    }
    Ok(())
}

async fn run(options: &Options, locations: &Locations) -> Result<(), Error> {
    let mut client = Client::connect(&locations.control_socket()).await?;
    match &options.command {
        Command::Upgrade { .. }
        | Command::RefreshUpdateCache
        | Command::Permissions { action: Some(_) } => unreachable!("handled in main"),
        Command::Permissions { action: None } => {
            let held: Permissions = client.request(Request::new(types::PERMISSIONS, 0)).await?;
            print!("{}", permissions_report(&held));
        }
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
            print!("{}", status_next_steps(s.initialised));
        }
        Command::Devices => {
            let d: Devices = client.request(Request::new(types::DEVICES, 0)).await?;
            if d.devices.is_empty() {
                println!("no devices");
            }
            let groups = ligatured::control::group_devices(&d.devices);
            let any_other = groups.iter().any(|g| !g.this_device() && !g.revoked());
            for group in groups {
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
            print!("{}", devices_next_steps(any_other));
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
                Some(e) => return Err(pair_refusal(offer.is_none(), &e)),
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
                    eprintln!("(file {f})");
                }
                for f in &c.promised_files {
                    eprintln!("(file {f}, promised)");
                }
            }
            ClipboardAction::Set { text } => {
                let mut req = Request::new(types::CLIPBOARD_SET, 0);
                req.text = Some(text.clone());
                let _: Empty = client.request(req).await?;
            }
            ClipboardAction::Put { mime, path } => {
                let mut req = Request::new(types::CLIPBOARD_PUT, 0);
                req.mime = Some(mime.clone());
                req.paths = Some(vec![absolute(path)?]);
                let _: Empty = client.request(req).await?;
            }
            ClipboardAction::Paste { mime, out } => {
                let mut req = Request::new(types::CLIPBOARD_PASTE, 0);
                req.mime = Some(mime.clone());
                req.paths = Some(vec![absolute(out)?]);
                let pasted: ClipboardPasted = client.request(req).await?;
                eprintln!("{} written to {}", human(pasted.size), out.display());
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
        Command::Input { action } => input(&mut client, action).await?,
        Command::Screen { action } => screen(&mut client, action).await?,
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

/// `lpctl input`.
async fn input(client: &mut Client, action: &InputAction) -> Result<(), Error> {
    match action {
        InputAction::Layout { action: None } => {
            let layout: InputLayout = client.request(Request::new(types::INPUT_LAYOUT, 0)).await?;
            if layout.layout.is_empty() {
                println!("no edge leads anywhere");
            }
            let edges = layout.layout.len();
            let known: Devices = client.request(Request::new(types::DEVICES, 1)).await?;
            for entry in layout.layout {
                let name = known
                    .devices
                    .iter()
                    .find(|d| d.key == entry.device)
                    .and_then(|d| d.display_name.clone())
                    .unwrap_or_else(|| "?".to_owned());
                println!(
                    "{:<7} {:<20} {}…  {}",
                    entry.edge,
                    name,
                    entry.device.fingerprint(),
                    entry.device
                );
            }
            print!("{}", layout_next_steps(edges));
        }
        InputAction::Layout {
            action: Some(LayoutAction::Set { edge, device }),
        } => {
            let key = resolve_device(client, device).await?;
            let mut req = Request::new(types::INPUT_LAYOUT_SET, 0);
            req.edge = Some(edge.clone());
            req.key = Some(key);
            let _: Empty = client.request(req).await?;
            println!("the {edge} edge leads to {key}");
        }
        InputAction::Layout {
            action: Some(LayoutAction::Unset { edge }),
        } => {
            let mut req = Request::new(types::INPUT_LAYOUT_UNSET, 0);
            req.edge = Some(edge.clone());
            let _: Empty = client.request(req).await?;
            println!("the {edge} edge leads nowhere");
        }
        InputAction::Status => {
            let s: InputStatus = client.request(Request::new(types::INPUT_STATUS, 0)).await?;
            println!("accepts:   {}", s.accepts);
            println!("provides:  {}", s.provides);
            let driving = s.driving.is_some();
            match s.driving {
                Some(d) => println!(
                    "driving:   {} ({}…) {}x{}, {} batches{}",
                    d.peer,
                    d.peer.fingerprint(),
                    d.width,
                    d.height,
                    d.batches,
                    d.edge
                        .map(|e| format!(", by the {e} edge"))
                        .unwrap_or_default()
                ),
                None => println!("driving:   nothing"),
            }
            match s.driven_by {
                Some(p) => println!("driven by: {} ({}…)", p, p.fingerprint()),
                None => println!("driven by: nobody"),
            }
            if let Some(rt) = s.round_trip {
                println!(
                    "round trip: {:.1} ms median, {:.1} ms worst, {} samples",
                    rt.median_ms, rt.max_ms, rt.samples
                );
            }
            for entry in s.layout {
                println!(
                    "{:<7} -> {}…  {}",
                    entry.edge,
                    entry.device.fingerprint(),
                    entry.device
                );
            }
            print!("{}", input_status_next_steps(driving));
        }
        InputAction::Release => {
            let _: Empty = client
                .request(Request::new(types::INPUT_RELEASE, 0))
                .await?;
            println!("released.");
        }
    }
    Ok(())
}

/// `lpctl screen`.
async fn screen(client: &mut Client, action: &ScreenAction) -> Result<(), Error> {
    match action {
        ScreenAction::Displays { device } => {
            let key = resolve_device(client, device).await?;
            let mut req = Request::new(types::SCREEN_DISPLAYS, 0);
            req.key = Some(key);
            let d: ScreenDisplays = client.request(req).await?;
            for display in d.displays {
                println!(
                    "{:<12} {}x{} at {}x{}  {}",
                    display.id,
                    display.width,
                    display.height,
                    display.scale(),
                    if display.primary { "  primary" } else { "" },
                    display.name.unwrap_or_default()
                );
            }
        }
        ScreenAction::Start {
            device,
            display,
            width,
            height,
            rate,
            out,
            frames,
        } => {
            let key = resolve_device(client, device).await?;
            let mut req = Request::new(types::SCREEN_START, 0);
            req.key = Some(key);
            req.display = display.clone();
            req.width = Some(*width);
            req.height = Some(*height);
            req.frame_rate = Some(*rate);
            let mut file = std::fs::File::create(out)
                .map_err(|e| Error::Io(format!("{}: {e}", out.display())))?;
            let (mut count, mut bytes_total, mut asked_stop) = (0u64, 0u64, false);
            let response = client
                .call(req, |event: &Event| {
                    if let Some(started) = &event.started {
                        eprintln!(
                            "watching {} at {}x{} {} fps ({}); stream in {}",
                            started.display,
                            started.width,
                            started.height,
                            started.frame_rate,
                            started.format,
                            out.display()
                        );
                    }
                    if let Some(frame) = &event.frame {
                        let _ = file.write_all(&frame.bytes.0);
                        bytes_total += frame.bytes.0.len() as u64;
                        if frame.last {
                            count += 1;
                            if count % 30 == 0 {
                                eprintln!("{count} frames, {}", human(bytes_total));
                            }
                            if !asked_stop && frames.is_some_and(|n| count >= n) {
                                asked_stop = true;
                                let mut stop = Request::new(types::SCREEN_STOP, 0);
                                stop.key = Some(key);
                                return Some(stop);
                            }
                        }
                    }
                    None
                })
                .await?;
            let ended: ScreenStopped = response.into_ok()?;
            eprintln!(
                "share ended ({}): {count} frames, {}",
                ended.reason,
                human(bytes_total)
            );
        }
        ScreenAction::Stop { device } => {
            let mut req = Request::new(types::SCREEN_STOP, 0);
            if let Some(device) = device {
                req.key = Some(resolve_device(client, device).await?);
            }
            let _: Empty = client.request(req).await?;
            println!("stopped.");
        }
        ScreenAction::Status => {
            let s: ScreenStatus = client
                .request(Request::new(types::SCREEN_STATUS, 0))
                .await?;
            println!("shares: {}", s.shares);
            println!("views:  {}", s.views);
            let active = !s.sharing.is_empty() || !s.watching.is_empty();
            for share in s.sharing {
                println!(
                    "sharing {} to {}… at {}x{} {} fps, {} frames, {} dropped",
                    share.started.display,
                    share.peer.fingerprint(),
                    share.started.width,
                    share.started.height,
                    share.started.frame_rate,
                    share.frames,
                    share.dropped
                );
            }
            for watch in s.watching {
                println!(
                    "watching {} of {}… at {}x{} {} fps, {} frames",
                    watch.started.display,
                    watch.peer.fingerprint(),
                    watch.started.width,
                    watch.started.height,
                    watch.started.frame_rate,
                    watch.frames
                );
            }
            print!("{}", screen_status_next_steps(active));
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
            for f in &o.offers {
                println!("{}  {:<40} {:>10}  {}", f.id, f.name, human(f.size), f.mime);
            }
            println!("received files land in {}", o.location);
            print!("{}", offers_next_steps(o.offers.len()));
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
            let in_flight = t.transfers.len();
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
            print!("{}", transfers_next_steps(in_flight));
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
    if let Some(p) = lp_core::input::InputParams::from_descriptor(d) {
        return format!(
            "  accepts={} provides={} absolute={} text={}",
            p.accepts, p.provides, p.absolute, p.text
        );
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_permission_opens_its_own_page() {
        assert!(settings_url(Permission::Accessibility).ends_with("Privacy_Accessibility"));
        assert!(settings_url(Permission::InputMonitoring).ends_with("Privacy_ListenEvent"));
        assert!(settings_url(Permission::ScreenRecording).ends_with("Privacy_ScreenCapture"));
    }

    #[test]
    fn a_reset_covers_every_service_behind_the_three() {
        for service in ["Accessibility", "PostEvent", "ListenEvent", "ScreenCapture"] {
            assert!(TCC_SERVICES.contains(&service), "{service}");
        }
    }

    #[test]
    fn only_a_mac_has_permissions_to_manage() {
        assert_eq!(mac_only().is_ok(), cfg!(target_os = "macos"));
        if !cfg!(target_os = "macos") {
            assert!(permissions_open("accessibility").is_err());
            assert!(permissions_reset().is_err());
        }
    }

    #[test]
    fn the_permissions_report_says_how_each_missing_one_is_granted() {
        use ligatured::control::{PermissionState, Permissions};
        let state = |name: &str, label: &str, granted: bool| PermissionState {
            name: name.to_owned(),
            label: label.to_owned(),
            granted,
            enables: "something".to_owned(),
        };
        let held = Permissions {
            permissions: vec![
                state("accessibility", "Accessibility", true),
                state("input-monitoring", "Input Monitoring", false),
                state("screen-recording", "Screen Recording", false),
            ],
        };
        let report = permissions_report(&held);
        assert!(
            report.contains("Accessibility     granted  something\n"),
            "{report}"
        );
        assert!(
            report.contains("Input Monitoring  missing  something\n"),
            "{report}"
        );
        assert!(!report.contains("open accessibility"), "{report}");
        assert!(
            report.contains("lpctl permissions open input-monitoring"),
            "{report}"
        );
        assert!(
            report.contains("lpctl permissions open screen-recording"),
            "{report}"
        );
        assert!(report.contains("lpctl permissions reset"), "{report}");
        assert!(report.contains("restarts by itself"), "{report}");

        let all = Permissions {
            permissions: vec![state("accessibility", "Accessibility", true)],
        };
        let report = permissions_report(&all);
        assert!(report.contains("all granted"), "{report}");
        assert!(!report.contains("open "), "{report}");
        assert!(report.contains("lpctl permissions reset"), "{report}");

        let none = Permissions {
            permissions: vec![],
        };
        assert_eq!(
            permissions_report(&none),
            "this device needs no permissions\n"
        );
    }

    #[test]
    fn next_steps_align_the_commands_and_vanish_when_there_are_none() {
        assert_eq!(next_steps(&[]), "");
        let out = next_steps(&[("lpctl a", "does a"), ("lpctl longer", "does b")]);
        assert_eq!(out, "\n  lpctl a       does a\n  lpctl longer  does b\n");
    }

    #[test]
    fn every_listing_ends_with_the_commands_that_act_on_it() {
        let fresh = status_next_steps(false);
        assert!(fresh.contains("lpctl init --name <NAME>"), "{fresh}");
        assert!(fresh.contains("lpctl pair"), "{fresh}");
        let running = status_next_steps(true);
        assert!(!running.contains("init"), "{running}");
        assert!(running.contains("lpctl devices"), "{running}");
        assert!(running.contains("lpctl pair"), "{running}");

        assert!(devices_next_steps(false).contains("lpctl pair"));
        assert!(!devices_next_steps(false).contains("revoke"));
        assert!(devices_next_steps(true).contains("lpctl revoke <KEY>"));

        assert!(layout_next_steps(0).contains("lpctl input layout set <EDGE> <DEVICE>"));
        assert!(!layout_next_steps(0).contains("unset"));
        assert!(layout_next_steps(1).contains("lpctl input layout unset <EDGE>"));

        assert!(input_status_next_steps(false).contains("lpctl input layout"));
        assert!(!input_status_next_steps(false).contains("release"));
        assert!(input_status_next_steps(true).contains("lpctl input release"));

        let idle = screen_status_next_steps(false);
        assert!(idle.contains("lpview <DEVICE>"), "{idle}");
        assert!(idle.contains("lpctl screen displays <DEVICE>"), "{idle}");
        assert!(!idle.contains("stop"), "{idle}");
        assert!(screen_status_next_steps(true).contains("lpctl screen stop [DEVICE]"));

        assert!(offers_next_steps(0).contains("lpctl files offer <PATH>"));
        assert!(!offers_next_steps(0).contains("withdraw"));
        assert!(offers_next_steps(2).contains("lpctl files withdraw <ID>"));

        assert!(transfers_next_steps(0).contains("lpctl files send <DEVICE> <PATHS>..."));
        assert!(!transfers_next_steps(0).contains("cancel"));
        assert!(transfers_next_steps(1).contains("lpctl files cancel <TRANSFER>"));
    }

    #[test]
    fn a_join_on_an_initialised_device_says_how_a_device_is_added_instead() {
        use ligatured::control::ErrorBody;
        let refused = ErrorBody::new("already-initialised", "already initialised");
        let Error::Refused(text) = pair_refusal(true, &refused) else {
            panic!("a refusal")
        };
        assert!(text.contains("already has its keys"), "{text}");
        assert!(text.contains("lpctl pair <OFFER>"), "{text}");
        let Error::Refused(text) = pair_refusal(false, &refused) else {
            panic!("a refusal")
        };
        assert_eq!(text, "already initialised");
        let other = ErrorBody::new("not-found", "no such device");
        let Error::Refused(text) = pair_refusal(true, &other) else {
            panic!("a refusal")
        };
        assert_eq!(text, "no such device");
    }
}
