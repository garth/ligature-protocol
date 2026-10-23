//! The loopback end-to-end run (milestone 1 task G2): two real daemons with
//! separate data directories, driven only through `lpctl`, pair, share a
//! clipboard both ways, and revoke. The automated stand-in for the
//! two-machine run.

use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn lpctl() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lpctl"))
}

fn ligatured() -> PathBuf {
    let bin = lpctl().with_file_name(format!("ligatured{}", std::env::consts::EXE_SUFFIX));
    if !bin.exists() {
        // cargo test -p lpctl does not build the daemon; cargo test --workspace does.
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
        let status = Command::new(cargo)
            .args(["build", "-p", "ligatured"])
            .status()
            .expect("cargo build -p ligatured");
        assert!(status.success());
    }
    bin
}

struct Daemon {
    child: Child,
    data: PathBuf,
    socket: PathBuf,
    /// What `lpctl` is given to find the daemon: the explicit data directory,
    /// or the platform variables alone.
    ctl_env: Vec<(&'static str, PathBuf)>,
    log: Option<std::thread::JoinHandle<()>>,
}

impl Daemon {
    /// Starts a real daemon that touches nothing of the owner's: fake
    /// clipboard and notification backends, its own home (so its download
    /// folder is under the test's directory) and its own clipboard cache,
    /// with its log copied to the test's stderr under its name (LP-126).
    fn start(root: &Path, name: &str) -> Self {
        let data = root.join("data").join(name);
        let home = root.join("home").join(name);
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let mut command = Command::new(ligatured());
        command
            .env("LIGATURE_DATA_DIR", &data)
            .env("LIGATURE_CLIPBOARD_CACHE", home.join("cache"))
            .env("HOME", &home);
        Self::spawn(
            command,
            name,
            data.clone(),
            data.join("control.sock"),
            vec![("LIGATURE_DATA_DIR", data)],
        )
    }

    /// Starts a daemon told nothing Ligature-specific about where to live: it
    /// takes the platform defaults from `HOME`, `XDG_DATA_HOME` and
    /// `XDG_RUNTIME_DIR`, all under the test's directory, and `lpctl` is
    /// given the same three and nothing else.
    fn start_with_platform_defaults(root: &Path, name: &str) -> Self {
        let home = root.join("home").join(name);
        let xdg_data = root.join("xdg").join(name);
        let runtime = root.join("run").join(name);
        for d in [&home, &xdg_data, &runtime] {
            std::fs::create_dir_all(d).unwrap();
        }
        let mut command = Command::new(ligatured());
        command
            .env_remove("LIGATURE_DATA_DIR")
            .env_remove("LIGATURE_RUNTIME_DIR")
            .env_remove("LIGATURE_CLIPBOARD_CACHE")
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &xdg_data)
            .env("XDG_RUNTIME_DIR", &runtime);
        Self::spawn(
            command,
            name,
            xdg_data.join("ligature"),
            runtime.join("ligature").join("control.sock"),
            vec![
                ("HOME", home),
                ("XDG_DATA_HOME", xdg_data),
                ("XDG_RUNTIME_DIR", runtime),
            ],
        )
    }

    fn spawn(
        mut command: Command,
        name: &str,
        data: PathBuf,
        socket: PathBuf,
        ctl_env: Vec<(&'static str, PathBuf)>,
    ) -> Self {
        let mut child = command
            .args(["--device-name", name])
            .env("LIGATURE_FAKE_CLIPBOARD", "1")
            .env("LIGATURE_FAKE_NOTIFICATIONS", "1")
            .env("RUST_LOG", "info")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start ligatured");
        let stderr = child.stderr.take().unwrap();
        let tag = name.to_owned();
        let log = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                eprintln!("[{tag}] {line}");
            }
        });
        let d = Self {
            child,
            data,
            socket,
            ctl_env,
            log: Some(log),
        };
        d.wait_for_socket();
        d
    }

    fn wait_for_socket(&self) {
        let sock = &self.socket;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !sock.exists() {
            assert!(
                Instant::now() < deadline,
                "daemon did not create its socket"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn ctl(&self, args: &[&str]) -> std::process::Output {
        let mut command = Command::new(lpctl());
        command
            .env_remove("LIGATURE_DATA_DIR")
            .env_remove("LIGATURE_RUNTIME_DIR");
        for (name, value) in &self.ctl_env {
            command.env(name, value);
        }
        command.args(args).output().expect("run lpctl")
    }

    fn ctl_ok(&self, args: &[&str]) -> String {
        let out = self.ctl(args);
        assert!(
            out.status.success(),
            "lpctl {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(log) = self.log.take() {
            let _ = log.join();
        }
    }
}

/// Whether `status` output lists a peer by `name`. Other devices on the LAN
/// (the owner's own daemon, for one) connect as strangers and may appear in
/// the count, so the tests look for names, never exact counts.
fn lists_peer(status: &str, name: &str) -> bool {
    status
        .lines()
        .skip_while(|l| !l.starts_with("peers:"))
        .skip(1)
        .any(|l| l.contains(name))
}

fn until(what: &str, timeout: Duration, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn two_daemons_pair_share_the_clipboard_both_ways_and_revoke() {
    let root = tempfile::tempdir().unwrap();
    let a = Daemon::start(root.path(), "linux-box");
    let b = Daemon::start(root.path(), "mac-box");

    // A becomes Alice's first device.
    let init = a.ctl_ok(&["init", "--name", "Alice"]);
    assert!(init.contains("created person Alice"), "{init}");
    assert!(a.ctl_ok(&["status"]).contains("initialised: true"));
    assert!(b.ctl_ok(&["status"]).contains("initialised: false"));

    // B shows an offer; A introduces it. Both answer yes to the code.
    let mut join = Command::new(lpctl())
        .env("LIGATURE_DATA_DIR", &b.data)
        .args(["--yes", "pair"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = join.stdout.take().unwrap();
    let (offer_tx, offer_rx) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        use std::io::BufRead as _;
        let mut collected = String::new();
        for line in std::io::BufReader::new(stdout).lines() {
            let line = line.unwrap();
            if let Some(start) = line.find("lpctl pair '") {
                let rest = &line[start + "lpctl pair '".len()..];
                if let Some(end) = rest.find('\'') {
                    let _ = offer_tx.send(rest[..end].to_owned());
                }
            }
            collected.push_str(&line);
            collected.push('\n');
        }
        collected
    });
    let offer = offer_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the newcomer printed its offer");
    assert!(offer.starts_with("ligature:pair/1?"), "{offer}");
    let introduced = a.ctl_ok(&["--yes", "pair", &offer]);
    assert!(introduced.contains("paired."), "{introduced}");
    let status = join.wait().unwrap();
    let joined = reader.join().unwrap();
    assert!(status.success(), "newcomer side failed: {joined}");
    assert!(joined.contains("paired."), "{joined}");

    // Both list both devices with the right names: one row per device
    // with its binding and principal beneath (LP-84).
    let devices_a = a.ctl_ok(&["devices"]);
    let devices_b = b.ctl_ok(&["devices"]);
    for d in [&devices_a, &devices_b] {
        assert_eq!(d.matches("linux-box").count(), 1, "{d}");
        assert_eq!(d.matches("mac-box").count(), 1, "{d}");
        assert_eq!(d.lines().count(), 6, "{d}");
        assert_eq!(d.matches("  binding ").count(), 2, "{d}");
        assert_eq!(d.matches("  principal ").count(), 2, "{d}");
    }

    // They find each other over mDNS and connect.
    until("a and b connected", Duration::from_secs(30), || {
        lists_peer(&a.ctl_ok(&["status"]), "mac-box")
            && lists_peer(&b.ctl_ok(&["status"]), "linux-box")
    });
    until(
        "clipboard offered both ways",
        Duration::from_secs(10),
        || {
            let caps = a.ctl_ok(&["caps"]);
            caps.matches("org.ligatureprotocol.capability.clipboard")
                .count()
                >= 2
        },
    );

    // Clipboard: A to B, then B to A.
    a.ctl_ok(&["clipboard", "set", "copied on linux"]);
    until("text reaches b", Duration::from_secs(5), || {
        b.ctl_ok(&["clipboard", "get"]).trim() == "copied on linux"
    });
    b.ctl_ok(&["clipboard", "set", "copied on mac"]);
    until("text reaches a", Duration::from_secs(5), || {
        a.ctl_ok(&["clipboard", "get"]).trim() == "copied on mac"
    });

    // A revokes B: B is dropped and cannot come back.
    let b_key = b
        .ctl_ok(&["status"])
        .lines()
        .find_map(|l| l.strip_prefix("binding:").map(|k| k.trim().to_owned()))
        .unwrap();
    a.ctl_ok(&["revoke", &b_key]);
    assert!(a.ctl_ok(&["devices"]).contains("REVOKED"));
    until("b dropped by a", Duration::from_secs(30), || {
        !lists_peer(&a.ctl_ok(&["status"]), "mac-box")
    });
    // B redials every ten seconds; past one such tick, A still refuses it.
    std::thread::sleep(Duration::from_secs(12));
    assert!(!lists_peer(&a.ctl_ok(&["status"]), "mac-box"));
    drop(a);
    drop(b);
}

/// With no flag and no `LIGATURE_*` variable, the daemon lives in the
/// platform's directories and `lpctl` finds it there (LP-85).
#[test]
fn lpctl_finds_a_daemon_at_the_platform_defaults_with_no_flag_or_variable() {
    let root = tempfile::tempdir().unwrap();
    let d = Daemon::start_with_platform_defaults(root.path(), "plain-box");
    assert!(
        d.socket.starts_with(root.path().join("run")),
        "the socket is under XDG_RUNTIME_DIR: {}",
        d.socket.display()
    );
    assert!(d.ctl_ok(&["status"]).contains("initialised: false"));
    let init = d.ctl_ok(&["init", "--name", "Alice"]);
    assert!(init.contains("created person Alice"), "{init}");
    let status = d.ctl_ok(&["status"]);
    assert!(status.contains("initialised: true"), "{status}");
    assert!(status.contains("plain-box"), "{status}");
    assert!(
        d.data.join("store.sqlite").exists() && d.data.join("keys.cbor").exists(),
        "state under XDG_DATA_HOME/ligature: {}",
        d.data.display()
    );
    drop(d);
}
