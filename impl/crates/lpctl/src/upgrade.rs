//! `lpctl upgrade` and the once-a-day check for a new release (LP-140, ADR 0042).
//!
//! A release is verified before anything of it is used: the checksum file's
//! minisign signature against [`RELEASE_PUBLIC_KEY`], its trusted comment
//! against the version asked for, and the tarball against its checksum. Only
//! then is the tarball unpacked and its `install.sh` run, which replaces the
//! binaries and restarts the service. A version no newer than the running one
//! is never installed, so a replayed older release is refused.
//!
//! The daily check is the CLI's alone. An ordinary command reads a small file
//! in the data directory ([`CHECK_FILE`]); when it is a day old the command
//! stamps it and spawns a detached `lpctl refresh-update-cache`, which asks
//! the releases listing for the newest version and writes it there; and when
//! the file names a version newer than this binary, the command says so in
//! one line on stderr once it is done. Nothing happens when stderr is not a
//! terminal or under [`NO_CHECK_ENV`], and the daemon never checks.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use lp_core::Error;
use lp_core::paths::Locations;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;

/// The project's release key (ADR 0042), the one the install guide publishes.
pub const RELEASE_PUBLIC_KEY: &str = "RWRG8kOII0tLS+Ll66WZ15knTjMWMnxqIxyRfwJRiMB64wBRsGxJy2BJ";
/// The releases listing: GitHub's API, newest first.
pub const RELEASES_URL: &str =
    "https://api.github.com/repos/garth/ligature-protocol/releases?per_page=20";
/// Set to anything to switch the daily check off.
pub const NO_CHECK_ENV: &str = "LIGATURE_NO_UPDATE_CHECK";
/// The daily check's file, in the data directory.
pub const CHECK_FILE: &str = "update-check.json";
/// How long one check holds.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// The most a listing, a checksum file or a signature may be.
const SMALL_LIMIT: u64 = 4 * 1024 * 1024;
/// The most a tarball may be.
const TARBALL_LIMIT: u64 = 256 * 1024 * 1024;
/// How long a small request may take in all.
const SMALL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the download may take in all.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const USER_AGENT: &str = concat!(
    "lpctl/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/garth/ligature-protocol)"
);

/// Where releases are listed and which key signs them: GitHub and the
/// project's key, or a test's own.
#[derive(Debug, Clone)]
pub struct Source {
    /// The releases listing, in the GitHub API's shape.
    pub releases_url: String,
    /// The minisign public key that signs `SHA256SUMS`, base64.
    pub public_key: String,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            releases_url: RELEASES_URL.to_owned(),
            public_key: RELEASE_PUBLIC_KEY.to_owned(),
        }
    }
}

/// The version this binary is.
#[must_use]
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is semver")
}

/// One release of the listing.
#[derive(Debug, Clone)]
pub struct Release {
    /// The version, from the tag without its `v`.
    pub version: Version,
    /// Its files.
    pub assets: Vec<Asset>,
}

/// One file of a release.
#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    /// The file name, as `SHA256SUMS` lists it.
    pub name: String,
    /// Where to fetch it.
    #[serde(rename = "browser_download_url")]
    pub url: String,
}

#[derive(Deserialize)]
struct ListedRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

/// The newest release a running `current` may move to: never a draft, and a
/// pre-release only while `current` is one itself. `None` when the listing
/// has nothing eligible.
///
/// # Errors
///
/// Returns [`Error::Decode`] when the listing is not the API's JSON.
pub fn latest(listing: &str, current: &Version) -> Result<Option<Release>, Error> {
    let listed: Vec<ListedRelease> = serde_json::from_str(listing)
        .map_err(|e| Error::Decode(format!("releases listing: {e}")))?;
    let mut best: Option<Release> = None;
    for release in listed {
        if release.draft {
            continue;
        }
        let Some(tag) = release.tag_name.strip_prefix('v') else {
            continue;
        };
        let Ok(version) = Version::parse(tag) else {
            continue;
        };
        if !version.pre.is_empty() && current.pre.is_empty() {
            continue;
        }
        if best.as_ref().is_none_or(|b| version > b.version) {
            best = Some(Release {
                version,
                assets: release.assets,
            });
        }
    }
    Ok(best)
}

/// The tarball's name for a platform: `linux` with the machine's architecture,
/// `macos-universal` on any Mac, nothing for a platform without a package.
#[must_use]
pub fn asset_name(version: &Version, os: &str, arch: &str) -> Option<String> {
    let suffix = match (os, arch) {
        ("linux", "x86_64" | "aarch64") => format!("linux-{arch}"),
        ("macos", _) => "macos-universal".to_owned(),
        _ => return None,
    };
    Some(format!("ligature-{version}-{suffix}.tar.gz"))
}

/// Checks a release's checksum file before it is read: the minisign signature
/// against `public_key` and the trusted comment against `version`, so one
/// release's checksums cannot stand in for another's.
///
/// # Errors
///
/// Returns [`Error::InvalidKey`] when the key does not decode and
/// [`Error::InvalidSignature`] when the signature does not decode, does not
/// verify, or is for another version.
pub fn verify_checksums(
    sums: &[u8],
    signature: &str,
    public_key: &str,
    version: &Version,
) -> Result<(), Error> {
    let key = minisign_verify::PublicKey::from_base64(public_key)
        .map_err(|e| Error::InvalidKey(format!("release key: {e}")))?;
    let signature = minisign_verify::Signature::decode(signature)
        .map_err(|e| Error::InvalidSignature(format!("SHA256SUMS.minisig: {e}")))?;
    key.verify(sums, &signature, false).map_err(|e| {
        Error::InvalidSignature(format!(
            "SHA256SUMS does not verify against the release key: {e}"
        ))
    })?;
    let expected = format!("ligature-protocol v{version}");
    if signature.trusted_comment() != expected {
        return Err(Error::InvalidSignature(format!(
            "SHA256SUMS is signed for {:?}, not {expected:?}",
            signature.trusted_comment()
        )));
    }
    Ok(())
}

/// The checksum `SHA256SUMS` gives for `name`.
///
/// # Errors
///
/// Returns [`Error::Decode`] when the file has no line for `name`, or one
/// that is not a hash.
pub fn checksum_of(sums: &str, name: &str) -> Result<[u8; 32], Error> {
    for line in sums.lines() {
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(file)) = (parts.next(), parts.next()) else {
            continue;
        };
        if file.trim_start_matches('*') == name {
            let bytes = hex::decode(hash)
                .map_err(|e| Error::Decode(format!("SHA256SUMS line for {name}: {e}")))?;
            return <[u8; 32]>::try_from(bytes)
                .map_err(|_| Error::Decode(format!("SHA256SUMS line for {name}: not SHA-256")));
        }
    }
    Err(Error::Decode(format!("{name} is not in SHA256SUMS")))
}

/// What `lpctl upgrade` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No release is newer than the running version.
    UpToDate,
    /// A newer release exists and was only reported (`--check`).
    Available(Version),
    /// A newer release was verified and its `install.sh` ran.
    Installed(Version),
}

/// The upgrade: a source, the running version and the platform to fetch for.
#[derive(Debug, Clone)]
pub struct Upgrader {
    /// Where releases come from.
    pub source: Source,
    /// The version running now.
    pub current: Version,
    /// `std::env::consts::OS`, or a test's.
    pub os: &'static str,
    /// `std::env::consts::ARCH`, or a test's.
    pub arch: &'static str,
}

impl Upgrader {
    /// The upgrade of this binary's platform from the project's releases.
    #[must_use]
    pub fn new(source: Source, current: Version) -> Self {
        Self {
            source,
            current,
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        }
    }

    /// Finds the newest release and, unless `check_only`, verifies it,
    /// unpacks it and runs its `install.sh`, with progress and the
    /// installer's output on `out`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] when a request fails, the verification
    /// errors of [`verify_checksums`] and [`checksum_of`], and [`Error::Io`]
    /// when the tarball does not unpack or `install.sh` fails.
    pub fn run(&self, check_only: bool, out: &mut dyn Write) -> Result<Outcome, Error> {
        let agent = agent();
        let listing = get(
            &agent,
            &self.source.releases_url,
            SMALL_LIMIT,
            SMALL_TIMEOUT,
            "application/vnd.github+json",
        )?;
        let listing = String::from_utf8(listing)
            .map_err(|e| Error::Decode(format!("releases listing: {e}")))?;
        let Some(release) = latest(&listing, &self.current)? else {
            return Ok(Outcome::UpToDate);
        };
        if release.version <= self.current {
            return Ok(Outcome::UpToDate);
        }
        if check_only {
            return Ok(Outcome::Available(release.version));
        }
        let name = asset_name(&release.version, self.os, self.arch)
            .ok_or(Error::Unimplemented("a release package for this platform"))?;
        let url = |file: &str| {
            release
                .assets
                .iter()
                .find(|a| a.name == file)
                .map(|a| a.url.clone())
                .ok_or_else(|| Error::Decode(format!("release {} has no {file}", release.version)))
        };
        write_line(
            out,
            &format!("found {}; fetching its checksums", release.version),
        )?;
        let sums = get(
            &agent,
            &url("SHA256SUMS")?,
            SMALL_LIMIT,
            SMALL_TIMEOUT,
            "*/*",
        )?;
        let signature = get(
            &agent,
            &url("SHA256SUMS.minisig")?,
            SMALL_LIMIT,
            SMALL_TIMEOUT,
            "*/*",
        )?;
        let signature = String::from_utf8(signature)
            .map_err(|e| Error::InvalidSignature(format!("SHA256SUMS.minisig: {e}")))?;
        verify_checksums(&sums, &signature, &self.source.public_key, &release.version)?;
        let sums =
            String::from_utf8(sums).map_err(|e| Error::Decode(format!("SHA256SUMS: {e}")))?;
        let expected = checksum_of(&sums, &name)?;
        write_line(out, &format!("downloading {name}"))?;
        let tarball = get(&agent, &url(&name)?, TARBALL_LIMIT, DOWNLOAD_TIMEOUT, "*/*")?;
        let actual: [u8; 32] = Sha256::digest(&tarball).into();
        if actual != expected {
            return Err(Error::InvalidSignature(format!(
                "{name} does not match its checksum"
            )));
        }
        let dir = tempfile::tempdir().map_err(|e| Error::Io(e.to_string()))?;
        std::fs::write(dir.path().join(&name), &tarball).map_err(|e| Error::Io(e.to_string()))?;
        let unpacked = unpack(dir.path(), &name)?;
        write_line(out, "running its install.sh")?;
        let output = Command::new("sh")
            .arg("install.sh")
            .current_dir(&unpacked)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| Error::Io(format!("install.sh: {e}")))?;
        out.write_all(&output.stdout)
            .and_then(|()| out.write_all(&output.stderr))
            .map_err(|e| Error::Io(e.to_string()))?;
        if !output.status.success() {
            return Err(Error::Io(format!("install.sh failed ({})", output.status)));
        }
        Ok(Outcome::Installed(release.version))
    }
}

/// Unpacks `dir/name` with the system's `tar` and returns the directory it
/// held, which `install.sh` expects to run from.
fn unpack(dir: &Path, name: &str) -> Result<PathBuf, Error> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(name)
        .current_dir(dir)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| Error::Io(format!("tar: {e}")))?;
    if !status.success() {
        return Err(Error::Io(format!("tar failed on {name} ({status})")));
    }
    let unpacked = dir.join(name.trim_end_matches(".tar.gz"));
    if !unpacked.join("install.sh").is_file() {
        return Err(Error::Io(format!("{name} holds no install.sh")));
    }
    Ok(unpacked)
}

fn write_line(out: &mut dyn Write, line: &str) -> Result<(), Error> {
    writeln!(out, "{line}").map_err(|e| Error::Io(e.to_string()))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(20)))
        .build()
        .new_agent()
}

/// One GET, bounded in size and time. A status outside 2xx is an error.
fn get(
    agent: &ureq::Agent,
    url: &str,
    limit: u64,
    timeout: Duration,
    accept: &str,
) -> Result<Vec<u8>, Error> {
    let mut response = agent
        .get(url)
        .config()
        .timeout_global(Some(timeout))
        .build()
        .header("Accept", accept)
        .call()
        .map_err(|e| Error::Transport(format!("{url}: {e}")))?;
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| Error::Transport(format!("{url}: {e}")))
}

/// The daily check's file: when it last ran and the newest version it found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// When the listing was last asked, or last due to be asked.
    #[serde(with = "time::serde::rfc3339")]
    pub checked: OffsetDateTime,
    /// The newest eligible version the listing had, if it had one.
    pub latest: Option<String>,
}

/// What an ordinary command does about the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The file is missing or a day old: stamp it and spawn a refresh.
    pub refresh: bool,
    /// The file names a version newer than the running one: say so.
    pub newer: Option<Version>,
}

/// Decides from the check file alone, so it can be tested without a clock.
#[must_use]
pub fn decide(check: Option<&Check>, now: OffsetDateTime, current: &Version) -> Decision {
    let Some(check) = check else {
        return Decision {
            refresh: true,
            newer: None,
        };
    };
    let newer = check
        .latest
        .as_deref()
        .and_then(|v| Version::parse(v).ok())
        .filter(|v| v > current);
    Decision {
        refresh: now - check.checked >= CHECK_EVERY,
        newer,
    }
}

/// Reads the check file; anything missing or unreadable counts as no check.
#[must_use]
pub fn read_check(path: &Path) -> Option<Check> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes the check file, making the data directory if it is not there yet.
///
/// # Errors
///
/// Returns [`Error::Io`] when the directory or the file cannot be written.
pub fn write_check(path: &Path, check: &Check) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io(e.to_string()))?;
    }
    let text = serde_json::to_string(check).map_err(|e| Error::Io(e.to_string()))?;
    std::fs::write(path, text).map_err(|e| Error::Io(e.to_string()))
}

/// The start of an ordinary command: spawns the refresh when one is due and
/// returns the newer version to mention once the command is done, if the
/// check file names one. Does nothing under [`NO_CHECK_ENV`] or when stderr
/// is not a terminal.
#[must_use]
pub fn begin(locations: &Locations, current: &Version) -> Option<Version> {
    use std::io::IsTerminal as _;
    if std::env::var_os(NO_CHECK_ENV).is_some() || !std::io::stderr().is_terminal() {
        return None;
    }
    let path = locations.data_dir.join(CHECK_FILE);
    let check = read_check(&path);
    let now = OffsetDateTime::now_utc();
    let decision = decide(check.as_ref(), now, current);
    if decision.refresh {
        // Stamp first, so a burst of commands spawns one refresh; the refresh
        // overwrites the stamp with what it finds.
        let stamped = Check {
            checked: now,
            latest: check.and_then(|c| c.latest),
        };
        if write_check(&path, &stamped).is_ok() {
            spawn_refresh(&locations.data_dir);
        }
    }
    decision.newer
}

/// The line an ordinary command ends with when a newer version is known.
#[must_use]
pub fn notice(newer: &Version, current: &Version) -> String {
    format!(
        "lpctl: Ligature Protocol {newer} is available (this device has {current}); \
         `lpctl upgrade` installs it, and your other devices need it too"
    )
}

/// Runs `lpctl refresh-update-cache --data-dir DIR` detached: its own process
/// group, no terminal, not waited for.
fn spawn_refresh(data_dir: &Path) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = Command::new(exe);
    command
        .arg("refresh-update-cache")
        .arg("--data-dir")
        .arg(data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    drop(command.spawn());
}

/// `lpctl refresh-update-cache`: asks the listing for the newest eligible
/// version and writes the check file.
///
/// # Errors
///
/// Returns [`Error::Transport`] when the listing cannot be fetched,
/// [`Error::Decode`] when it does not parse and [`Error::Io`] when the file
/// cannot be written.
pub fn refresh(source: &Source, locations: &Locations, current: &Version) -> Result<(), Error> {
    let listing = get(
        &agent(),
        &source.releases_url,
        SMALL_LIMIT,
        SMALL_TIMEOUT,
        "application/vnd.github+json",
    )?;
    let listing =
        String::from_utf8(listing).map_err(|e| Error::Decode(format!("releases listing: {e}")))?;
    let latest = latest(&listing, current)?.map(|r| r.version.to_string());
    write_check(
        &locations.data_dir.join(CHECK_FILE),
        &Check {
            checked: OffsetDateTime::now_utc(),
            latest,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{BufRead as _, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use base64_lite::encode;
    use blake2::Blake2b512;
    use ed25519_dalek::{Signer as _, SigningKey};

    use super::*;

    /// Base64 without another dependency: the standard alphabet with padding.
    mod base64_lite {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

        pub fn encode(bytes: &[u8]) -> String {
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let b = [
                    chunk[0],
                    chunk.get(1).copied().unwrap_or(0),
                    chunk.get(2).copied().unwrap_or(0),
                ];
                let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
                for i in 0..4 {
                    if i <= chunk.len() {
                        out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
                    } else {
                        out.push('=');
                    }
                }
            }
            out
        }
    }

    /// A minisign key pair made in the test, signing as minisign does:
    /// Ed25519 over the BLAKE2b-512 of the file, then over the signature and
    /// the trusted comment.
    struct TestKey {
        key: SigningKey,
        id: [u8; 8],
    }

    impl TestKey {
        fn new(seed: u8) -> Self {
            Self {
                key: SigningKey::from_bytes(&[seed; 32]),
                id: [seed, 2, 3, 4, 5, 6, 7, 8],
            }
        }

        fn public(&self) -> String {
            let mut bin = b"Ed".to_vec();
            bin.extend_from_slice(&self.id);
            bin.extend_from_slice(&self.key.verifying_key().to_bytes());
            encode(&bin)
        }

        fn sign(&self, file: &[u8], trusted: &str) -> String {
            let digest = Blake2b512::digest(file);
            let signature = self.key.sign(&digest).to_bytes();
            let mut bin = b"ED".to_vec();
            bin.extend_from_slice(&self.id);
            bin.extend_from_slice(&signature);
            let mut global = signature.to_vec();
            global.extend_from_slice(trusted.as_bytes());
            let global = self.key.sign(&global).to_bytes();
            format!(
                "untrusted comment: test\n{}\ntrusted comment: {trusted}\n{}\n",
                encode(&bin),
                encode(&global)
            )
        }
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    /// Signed by the release key with rsign2 (LP-140), so the test knows the
    /// compiled-in key is the one that signs releases.
    const REAL_SUMS: &str = "0000000000000000000000000000000000000000000000000000000000000001  \
                             ligature-0.1.1-alpha-linux-x86_64.tar.gz\n\
                             0000000000000000000000000000000000000000000000000000000000000002  \
                             ligature-0.1.1-alpha-android.apk\n";
    const REAL_SIG: &str = "untrusted comment: signature from rsign secret key\n\
        RURG8kOII0tLSzBQhpn+nfWoMb4PLmAoIpTpaVVOPt4ytHf4a+NpmllQGnb/LzbWgJYBB4ncQF6jO0UndNq8PISoRIPCIIPALg4=\n\
        trusted comment: ligature-protocol v0.1.1-alpha\n\
        12o9XiaaNHJ5cIxKaYTiDW0evhAd4bWBQwEoVtmp4F0fRlTb5URol3ug3gCgDc7feO/UkdaBuv+uUkBBskDjBw==\n";

    #[test]
    fn the_compiled_in_key_verifies_a_real_signature() {
        verify_checksums(
            REAL_SUMS.as_bytes(),
            REAL_SIG,
            RELEASE_PUBLIC_KEY,
            &v("0.1.1-alpha"),
        )
        .unwrap();
        assert!(matches!(
            verify_checksums(
                REAL_SUMS.as_bytes(),
                REAL_SIG,
                RELEASE_PUBLIC_KEY,
                &v("0.1.2-alpha")
            ),
            Err(Error::InvalidSignature(_))
        ));
        let mut tampered = REAL_SUMS.to_owned();
        tampered.replace_range(0..1, "f");
        assert!(matches!(
            verify_checksums(
                tampered.as_bytes(),
                REAL_SIG,
                RELEASE_PUBLIC_KEY,
                &v("0.1.1-alpha")
            ),
            Err(Error::InvalidSignature(_))
        ));
    }

    #[test]
    fn a_test_key_signs_as_minisign_does() {
        let key = TestKey::new(1);
        let sig = key.sign(b"hello\n", "ligature-protocol v1.2.3");
        verify_checksums(b"hello\n", &sig, &key.public(), &v("1.2.3")).unwrap();
        let other = TestKey::new(2);
        assert!(matches!(
            verify_checksums(b"hello\n", &sig, &other.public(), &v("1.2.3")),
            Err(Error::InvalidSignature(_))
        ));
    }

    #[test]
    fn checksum_lines() {
        let sums = "ab".repeat(32) + "  a.tar.gz\n" + &"cd".repeat(32) + " *b.apk\n";
        assert_eq!(checksum_of(&sums, "a.tar.gz").unwrap(), [0xab; 32]);
        assert_eq!(checksum_of(&sums, "b.apk").unwrap(), [0xcd; 32]);
        assert!(matches!(checksum_of(&sums, "c"), Err(Error::Decode(_))));
        assert!(matches!(
            checksum_of("zz  a.tar.gz\n", "a.tar.gz"),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn asset_names() {
        let ver = v("0.2.0-alpha");
        assert_eq!(
            asset_name(&ver, "linux", "x86_64").unwrap(),
            "ligature-0.2.0-alpha-linux-x86_64.tar.gz"
        );
        assert_eq!(
            asset_name(&ver, "linux", "aarch64").unwrap(),
            "ligature-0.2.0-alpha-linux-aarch64.tar.gz"
        );
        assert_eq!(
            asset_name(&ver, "macos", "aarch64").unwrap(),
            "ligature-0.2.0-alpha-macos-universal.tar.gz"
        );
        assert_eq!(asset_name(&ver, "linux", "riscv64"), None);
        assert_eq!(asset_name(&ver, "windows", "x86_64"), None);
    }

    fn listing(entries: &[(&str, bool)]) -> String {
        let items: Vec<String> = entries
            .iter()
            .map(|(tag, draft)| {
                format!(
                    r#"{{"tag_name":"{tag}","draft":{draft},"prerelease":true,"assets":[{{"name":"SHA256SUMS","browser_download_url":"http://x/{tag}/SHA256SUMS"}}]}}"#
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    #[test]
    fn latest_picks_the_highest_eligible_release() {
        let l = listing(&[
            ("v0.1.0-alpha", false),
            ("v0.3.0-alpha", true),
            ("v0.2.0-alpha", false),
            ("not-a-version", false),
            ("v0.1.1-alpha", false),
        ]);
        let found = latest(&l, &v("0.1.0-alpha")).unwrap().unwrap();
        assert_eq!(found.version, v("0.2.0-alpha"));
        assert_eq!(found.assets[0].url, "http://x/v0.2.0-alpha/SHA256SUMS");
        // A stable version never moves to a pre-release.
        assert!(latest(&l, &v("0.1.0")).unwrap().is_none());
        let with_stable = listing(&[("v0.2.0-alpha", false), ("v0.1.5", false)]);
        assert_eq!(
            latest(&with_stable, &v("0.1.0")).unwrap().unwrap().version,
            v("0.1.5")
        );
        assert_eq!(
            latest(&with_stable, &v("0.1.0-alpha"))
                .unwrap()
                .unwrap()
                .version,
            v("0.2.0-alpha")
        );
        assert!(latest("[]", &v("0.1.0")).unwrap().is_none());
        assert!(matches!(
            latest("nonsense", &v("0.1.0")),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn decisions() {
        let now = OffsetDateTime::now_utc();
        let current = v("0.1.1-alpha");
        assert_eq!(
            decide(None, now, &current),
            Decision {
                refresh: true,
                newer: None
            }
        );
        let fresh_newer = Check {
            checked: now - Duration::from_secs(3600),
            latest: Some("0.2.0-alpha".to_owned()),
        };
        assert_eq!(
            decide(Some(&fresh_newer), now, &current),
            Decision {
                refresh: false,
                newer: Some(v("0.2.0-alpha"))
            }
        );
        let stale_same = Check {
            checked: now - CHECK_EVERY,
            latest: Some("0.1.1-alpha".to_owned()),
        };
        assert_eq!(
            decide(Some(&stale_same), now, &current),
            Decision {
                refresh: true,
                newer: None
            }
        );
        let stale_older = Check {
            checked: now - CHECK_EVERY * 3,
            latest: Some("0.1.0-alpha".to_owned()),
        };
        assert_eq!(
            decide(Some(&stale_older), now, &current),
            Decision {
                refresh: true,
                newer: None
            }
        );
        let fresh_unknown = Check {
            checked: now,
            latest: None,
        };
        assert_eq!(
            decide(Some(&fresh_unknown), now, &current),
            Decision {
                refresh: false,
                newer: None
            }
        );
    }

    #[test]
    fn the_check_file_round_trips_and_tolerates_rubbish() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join(CHECK_FILE);
        assert!(read_check(&path).is_none());
        let check = Check {
            checked: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            latest: Some("0.2.0-alpha".to_owned()),
        };
        write_check(&path, &check).unwrap();
        assert_eq!(read_check(&path).unwrap(), check);
        std::fs::write(&path, "{not json").unwrap();
        assert!(read_check(&path).is_none());
    }

    /// A fake release on a loopback server: a tarball whose `install.sh`
    /// writes `marker`, its checksums signed by `key` with `comment`, and a
    /// listing naming the version. `tamper` changes the tarball after it was
    /// summed.
    struct FakeRelease {
        base: String,
        marker: PathBuf,
    }

    fn fake_release(
        dir: &Path,
        version: &str,
        key: &TestKey,
        comment: &str,
        tamper: bool,
    ) -> FakeRelease {
        let name = format!("ligature-{version}-linux-x86_64");
        let unpacked = dir.join(&name);
        std::fs::create_dir_all(&unpacked).unwrap();
        let marker = dir.join("install-ran");
        std::fs::write(
            unpacked.join("install.sh"),
            format!(
                "#!/bin/sh\necho installing from \"$PWD\"\necho \"$PWD\" > '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        let tarball = format!("{name}.tar.gz");
        let status = Command::new("tar")
            .args(["-czf", &tarball, &name])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success());
        let mut bytes = std::fs::read(dir.join(&tarball)).unwrap();
        let sums = format!("{}  {tarball}\n", hex::encode(Sha256::digest(&bytes)));
        if tamper {
            bytes.push(0);
        }
        let sig = key.sign(sums.as_bytes(), comment);
        let mut routes = HashMap::new();
        routes.insert(format!("/dl/{tarball}"), bytes);
        routes.insert("/dl/SHA256SUMS".to_owned(), sums.into_bytes());
        routes.insert("/dl/SHA256SUMS.minisig".to_owned(), sig.into_bytes());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let listing = format!(
            r#"[{{"tag_name":"v{version}","draft":false,"prerelease":true,"assets":[
                {{"name":"{tarball}","browser_download_url":"{base}/dl/{tarball}"}},
                {{"name":"SHA256SUMS","browser_download_url":"{base}/dl/SHA256SUMS"}},
                {{"name":"SHA256SUMS.minisig","browser_download_url":"{base}/dl/SHA256SUMS.minisig"}}]}}]"#
        );
        routes.insert("/releases".to_owned(), listing.into_bytes());
        serve_on(listener, routes);
        FakeRelease { base, marker }
    }

    /// A loopback HTTP server for the files of a fake release, keyed by path.
    fn serve_on(listener: TcpListener, routes: HashMap<String, Vec<u8>>) {
        let routes = Arc::new(Mutex::new(routes));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let routes = Arc::clone(&routes);
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                            break;
                        }
                    }
                    let body = routes.lock().unwrap().get(&path).cloned();
                    let mut stream = reader.into_inner();
                    let response = match body {
                        Some(body) => {
                            let mut r = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .into_bytes();
                            r.extend_from_slice(&body);
                            r
                        }
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    drop(stream.write_all(&response));
                    drop(stream.flush());
                });
            }
        });
    }

    fn upgrader(release: &FakeRelease, key: &TestKey, current: &str) -> Upgrader {
        Upgrader {
            source: Source {
                releases_url: format!("{}/releases", release.base),
                public_key: key.public(),
            },
            current: v(current),
            os: "linux",
            arch: "x86_64",
        }
    }

    #[test]
    fn a_verified_release_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let key = TestKey::new(1);
        let release = fake_release(
            dir.path(),
            "0.9.0-alpha",
            &key,
            "ligature-protocol v0.9.0-alpha",
            false,
        );
        let mut out = Vec::new();
        let outcome = upgrader(&release, &key, "0.1.1-alpha")
            .run(false, &mut out)
            .unwrap();
        assert_eq!(outcome, Outcome::Installed(v("0.9.0-alpha")));
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains("downloading ligature-0.9.0-alpha-linux-x86_64.tar.gz"),
            "{out}"
        );
        assert!(out.contains("installing from"), "{out}");
        let ran_in = std::fs::read_to_string(&release.marker).unwrap();
        assert!(
            ran_in.trim().ends_with("ligature-0.9.0-alpha-linux-x86_64"),
            "{ran_in}"
        );
    }

    #[test]
    fn check_only_reports_and_downloads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let key = TestKey::new(1);
        // Signed for the wrong version: a download would fail, a check does not.
        let release = fake_release(dir.path(), "0.9.0-alpha", &key, "wrong", false);
        let mut out = Vec::new();
        let outcome = upgrader(&release, &key, "0.1.1-alpha")
            .run(true, &mut out)
            .unwrap();
        assert_eq!(outcome, Outcome::Available(v("0.9.0-alpha")));
        assert!(!release.marker.exists());
    }

    #[test]
    fn an_older_or_equal_release_is_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let key = TestKey::new(1);
        let release = fake_release(
            dir.path(),
            "0.9.0-alpha",
            &key,
            "ligature-protocol v0.9.0-alpha",
            false,
        );
        let mut out = Vec::new();
        assert_eq!(
            upgrader(&release, &key, "0.9.0-alpha")
                .run(false, &mut out)
                .unwrap(),
            Outcome::UpToDate
        );
        assert_eq!(
            upgrader(&release, &key, "1.0.0-alpha")
                .run(false, &mut out)
                .unwrap(),
            Outcome::UpToDate
        );
        assert!(!release.marker.exists());
    }

    #[test]
    fn a_release_signed_by_another_key_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let key = TestKey::new(1);
        let release = fake_release(
            dir.path(),
            "0.9.0-alpha",
            &key,
            "ligature-protocol v0.9.0-alpha",
            false,
        );
        let mut out = Vec::new();
        let err = upgrader(&release, &TestKey::new(2), "0.1.1-alpha")
            .run(false, &mut out)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidSignature(_)), "{err}");
        assert!(!release.marker.exists());
    }

    #[test]
    fn checksums_for_another_version_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let key = TestKey::new(1);
        let release = fake_release(
            dir.path(),
            "0.9.0-alpha",
            &key,
            "ligature-protocol v0.8.0-alpha",
            false,
        );
        let mut out = Vec::new();
        let err = upgrader(&release, &key, "0.1.1-alpha")
            .run(false, &mut out)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidSignature(_)), "{err}");
        assert!(!release.marker.exists());
    }

    #[test]
    fn a_tarball_that_does_not_match_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let key = TestKey::new(1);
        let release = fake_release(
            dir.path(),
            "0.9.0-alpha",
            &key,
            "ligature-protocol v0.9.0-alpha",
            true,
        );
        let mut out = Vec::new();
        let err = upgrader(&release, &key, "0.1.1-alpha")
            .run(false, &mut out)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidSignature(_)), "{err}");
        assert!(!release.marker.exists());
    }

    #[test]
    fn refresh_writes_what_the_listing_says() {
        let dir = tempfile::tempdir().unwrap();
        let key = TestKey::new(1);
        let release = fake_release(
            dir.path(),
            "0.9.0-alpha",
            &key,
            "ligature-protocol v0.9.0-alpha",
            false,
        );
        let data = dir.path().join("data");
        let locations = Locations {
            data_dir: data.clone(),
            runtime_dir: data.clone(),
        };
        let source = Source {
            releases_url: format!("{}/releases", release.base),
            public_key: key.public(),
        };
        refresh(&source, &locations, &v("0.1.1-alpha")).unwrap();
        let check = read_check(&data.join(CHECK_FILE)).unwrap();
        assert_eq!(check.latest.as_deref(), Some("0.9.0-alpha"));
        let decision = decide(Some(&check), OffsetDateTime::now_utc(), &v("0.1.1-alpha"));
        assert_eq!(decision.newer, Some(v("0.9.0-alpha")));
        assert!(!decision.refresh);
    }
}
