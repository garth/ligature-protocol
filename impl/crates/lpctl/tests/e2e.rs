//! The loopback end-to-end run (milestone 1 task G2): two real daemons with
//! separate data directories, driven only through `lpctl`, pair, share a
//! clipboard both ways, and revoke. The automated stand-in for the
//! two-machine run.

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
}

impl Daemon {
    fn start(data: &Path, name: &str) -> Self {
        std::fs::create_dir_all(data).unwrap();
        let child = Command::new(ligatured())
            .args(["--device-name", name])
            .env("LIGATURE_DATA_DIR", data)
            .env("LIGATURE_FAKE_CLIPBOARD", "1")
            .env("RUST_LOG", "info")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start ligatured");
        let d = Self {
            child,
            data: data.to_path_buf(),
        };
        d.wait_for_socket();
        d
    }

    fn wait_for_socket(&self) {
        let sock = self.data.join("control.sock");
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
        Command::new(lpctl())
            .env("LIGATURE_DATA_DIR", &self.data)
            .args(args)
            .output()
            .expect("run lpctl")
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
    }
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
    let a = Daemon::start(&root.path().join("data/a"), "linux-box");
    let b = Daemon::start(&root.path().join("data/b"), "mac-box");

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
        a.ctl_ok(&["status"]).contains("peers:       1")
            && b.ctl_ok(&["status"]).contains("peers:       1")
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
        a.ctl_ok(&["status"]).contains("peers:       0")
    });
    // Give B's redial a chance; A must still have no peer.
    std::thread::sleep(Duration::from_secs(4));
    assert!(a.ctl_ok(&["status"]).contains("peers:       0"));
    drop(a);
    drop(b);
}
