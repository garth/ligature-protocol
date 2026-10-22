//! Where received files land and what offered files are read from. Spec
//! chapter 05 section 3d; milestone 4 (LP-102).
//!
//! A [`FileStore`] is the receiving side's choice of destination: the
//! platform's download folder by default, a directory or path the requester
//! named, or memory for a capability inside the daemon. It hands out a
//! [`Landing`] per file that writes the bytes, hashes them as they pass, and
//! either moves the file into place or removes every trace of it. A
//! [`FileSource`] is what the sender reads: a path or bytes in memory. The
//! name rule and the download directory live here too.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use lp_core::Error;
use lp_core::files::FileDescriptor;
use lp_core::wire::{Bytes, Timestamp};
use lp_crypto::hash::Sha256Stream;

/// Environment variable naming the download directory, for tests and the
/// loopback run; the platform's folder otherwise.
pub const DOWNLOAD_DIR_ENV: &str = "LIGATURE_DOWNLOAD_DIR";

/// Suffix of a file still being written beside its destination.
pub const PART_SUFFIX: &str = ".ligature-part";

/// What the sender reads a file from.
#[derive(Debug, Clone)]
pub enum FileSource {
    /// A file on disk.
    Path(PathBuf),
    /// Bytes in memory.
    Bytes(Arc<Vec<u8>>),
}

impl FileSource {
    /// Opens the source for reading from the start.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if a path cannot be opened.
    pub async fn open(&self) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, Error> {
        match self {
            Self::Path(path) => {
                let file = tokio::fs::File::open(path)
                    .await
                    .map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
                Ok(Box::new(file))
            }
            Self::Bytes(bytes) => Ok(Box::new(std::io::Cursor::new(bytes.to_vec()))),
        }
    }

    /// Describes a file on disk: its base name, size and modification time,
    /// with a content type from its extension.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the path is not a readable regular file.
    pub fn describe_path(id: &str, path: &Path) -> Result<(FileDescriptor, Self), Error> {
        let meta =
            std::fs::metadata(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
        if !meta.is_file() {
            return Err(Error::Io(format!("{}: not a regular file", path.display())));
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_owned());
        let mut file = FileDescriptor::new(id, &safe_name(&name), meta.len());
        file.mime = mime_for(&name).to_owned();
        file.modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .and_then(|s| Timestamp::from_unix(s).ok());
        Ok((file, Self::Path(path.to_path_buf())))
    }
}

/// Where the receiving side puts a transfer's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// The platform's download folder, through the daemon's store.
    Downloads,
    /// A directory (the files land inside it under the name rule) or a file
    /// path (for a single file, refused if it exists).
    Path(PathBuf),
    /// Memory: the bytes come back in [`Landed::bytes`].
    Memory,
}

/// Where a file ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landed {
    /// The name it was given.
    pub name: String,
    /// Where it is, for display: a path on the desktops.
    pub location: Option<String>,
    /// The path on disk, when it landed on one.
    pub path: Option<PathBuf>,
    /// The bytes, when it landed in memory.
    pub bytes: Option<Vec<u8>>,
    /// SHA-256 of everything written, base64url unpadded.
    pub sha256: String,
    /// How many bytes were written.
    pub size: u64,
}

/// One file being written. Synchronous: the synchroniser drives it from a
/// blocking task so disk writes never hold a runtime thread.
pub trait Landing: Send {
    /// Appends `bytes`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`]; a full disk is reported as such in the message.
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error>;
    /// SHA-256 of everything written so far, base64url unpadded: compared
    /// with the sender's `end` before the file is put in place.
    fn digest(&self) -> String;
    /// Moves the file into place, applying `modified` where the platform
    /// allows, and says where it is.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if it cannot be put in place.
    fn finish(self: Box<Self>, modified: Option<Timestamp>) -> Result<Landed, Error>;
    /// Removes every trace of the file.
    fn abort(self: Box<Self>);
}

/// A place received files land by default. Implemented over a directory on
/// the desktops and over MediaStore on Android.
pub trait FileStore: Send + Sync {
    /// Whether files can land here right now; `receive` follows it.
    fn available(&self) -> bool;
    /// Starts landing `file` under the name rule.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if nothing can be created.
    fn open(&self, file: &FileDescriptor) -> Result<Box<dyn Landing>, Error>;
    /// Where this store puts files, for logs and `lpctl`.
    fn describe(&self) -> String;
}

/// A store over one directory: the download folder on the desktops.
#[derive(Debug, Clone)]
pub struct DirectoryStore {
    dir: PathBuf,
}

impl DirectoryStore {
    /// A store writing into `dir`, created on first use.
    #[must_use]
    pub fn new(dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { dir })
    }

    /// The directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Starts landing `file` in `dir` under the name rule, never
    /// overwriting.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the directory or the part file cannot be
    /// created.
    pub fn open_in(dir: &Path, file: &FileDescriptor) -> Result<Box<dyn Landing>, Error> {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
        let name = safe_name(&file.name);
        // Claim a part file whose final name is free; both are tried together
        // so two files of one name landing at once get different numbers.
        for attempt in 1..10_000u32 {
            let candidate = numbered(&name, attempt);
            let final_path = dir.join(&candidate);
            let part_path = dir.join(format!("{candidate}{PART_SUFFIX}"));
            if final_path.exists() {
                continue;
            }
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&part_path)
            {
                Ok(f) => {
                    return Ok(Box::new(DiskLanding {
                        file: Some(f),
                        part: part_path,
                        final_path,
                        name: candidate,
                        hasher: Sha256Stream::new(),
                        written: 0,
                    }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(Error::Io(format!("{}: {e}", part_path.display()))),
            }
        }
        Err(Error::Io(format!(
            "{}: no free name for {name}",
            dir.display()
        )))
    }

    /// Starts landing `file` at exactly `path`, refused if it exists.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the path exists or cannot be created.
    pub fn open_at(path: &Path, file: &FileDescriptor) -> Result<Box<dyn Landing>, Error> {
        if path.is_dir() {
            return Self::open_in(path, file);
        }
        if path.exists() {
            return Err(Error::Io(format!("{}: exists", path.display())));
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Io(format!("{}: {e}", parent.display())))?;
        }
        let part = PathBuf::from(format!("{}{PART_SUFFIX}", path.display()));
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(|e| Error::Io(format!("{}: {e}", part.display())))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.name.clone());
        Ok(Box::new(DiskLanding {
            file: Some(f),
            part,
            final_path: path.to_path_buf(),
            name,
            hasher: Sha256Stream::new(),
            written: 0,
        }))
    }
}

impl FileStore for DirectoryStore {
    fn available(&self) -> bool {
        // The directory may not exist yet; its parent must, and be writable
        // enough to create it in. Creating it here on every poll is cheap.
        std::fs::create_dir_all(&self.dir).is_ok()
            && std::fs::metadata(&self.dir).is_ok_and(|m| m.is_dir() && !m.permissions().readonly())
    }

    fn open(&self, file: &FileDescriptor) -> Result<Box<dyn Landing>, Error> {
        Self::open_in(&self.dir, file)
    }

    fn describe(&self) -> String {
        self.dir.display().to_string()
    }
}

/// A file being written beside its destination as a part file.
struct DiskLanding {
    file: Option<std::fs::File>,
    part: PathBuf,
    final_path: PathBuf,
    name: String,
    hasher: Sha256Stream,
    written: u64,
}

impl Landing for DiskLanding {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let Some(file) = self.file.as_mut() else {
            return Err(Error::Io("landing already closed".to_owned()));
        };
        file.write_all(bytes).map_err(|e| {
            if e.kind() == std::io::ErrorKind::StorageFull {
                Error::Io("no space".to_owned())
            } else {
                Error::Io(format!("{}: {e}", self.part.display()))
            }
        })?;
        self.hasher.update(bytes);
        self.written += bytes.len() as u64;
        Ok(())
    }

    fn digest(&self) -> String {
        Bytes(self.hasher.clone().finish().to_vec()).to_base64url()
    }

    fn finish(mut self: Box<Self>, modified: Option<Timestamp>) -> Result<Landed, Error> {
        let io = |p: &Path, e: std::io::Error| Error::Io(format!("{}: {e}", p.display()));
        if let Some(mut file) = self.file.take() {
            file.flush().map_err(|e| io(&self.part, e))?;
            if let Some(t) = modified
                && let Ok(secs) = u64::try_from(t.unix())
            {
                let _ = file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
            }
        }
        // The final name was free when the part file was claimed; a clash
        // that appeared since gets the next free number.
        let mut final_path = self.final_path.clone();
        let mut name = self.name.clone();
        if final_path.exists() {
            let dir = final_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let base = safe_name(&self.name);
            let mut n = 2;
            loop {
                let candidate = numbered(&base, n);
                let p = dir.join(&candidate);
                if !p.exists() {
                    final_path = p;
                    name = candidate;
                    break;
                }
                n += 1;
            }
        }
        std::fs::rename(&self.part, &final_path).map_err(|e| {
            let _ = std::fs::remove_file(&self.part);
            io(&final_path, e)
        })?;
        let sha256 = Bytes(self.hasher.clone().finish().to_vec()).to_base64url();
        Ok(Landed {
            name,
            location: Some(final_path.display().to_string()),
            path: Some(final_path),
            bytes: None,
            sha256,
            size: self.written,
        })
    }

    fn abort(mut self: Box<Self>) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.part);
    }
}

impl Drop for DiskLanding {
    fn drop(&mut self) {
        // A landing dropped without finish or abort (a panic, a lost task)
        // leaves no part file behind.
        if self.file.take().is_some() {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}

/// A file landing in memory.
#[derive(Debug, Default)]
pub struct MemoryLanding {
    name: String,
    buf: Vec<u8>,
    hasher: Sha256Stream,
}

impl MemoryLanding {
    /// An empty landing for `file`.
    #[must_use]
    pub fn open(file: &FileDescriptor) -> Box<dyn Landing> {
        Box::new(Self {
            name: safe_name(&file.name),
            buf: Vec::with_capacity(usize::try_from(file.size).unwrap_or(0).min(1 << 26)),
            hasher: Sha256Stream::new(),
        })
    }
}

impl Landing for MemoryLanding {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.buf.extend_from_slice(bytes);
        self.hasher.update(bytes);
        Ok(())
    }

    fn digest(&self) -> String {
        Bytes(self.hasher.clone().finish().to_vec()).to_base64url()
    }

    fn finish(self: Box<Self>, _modified: Option<Timestamp>) -> Result<Landed, Error> {
        let size = self.buf.len() as u64;
        Ok(Landed {
            name: self.name,
            location: None,
            path: None,
            bytes: Some(self.buf),
            sha256: Bytes(self.hasher.finish().to_vec()).to_base64url(),
            size,
        })
    }

    fn abort(self: Box<Self>) {}
}

/// The name rule: the last path component only, control characters and a
/// leading dot dropped, trimmed to 255 bytes keeping the extension, `file`
/// if nothing is left.
#[must_use]
pub fn safe_name(name: &str) -> String {
    let last = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    let trimmed = last.trim().trim_start_matches('.').trim();
    if trimmed.is_empty() || trimmed == ".." {
        return "file".to_owned();
    }
    if trimmed.len() <= 255 {
        return trimmed.to_owned();
    }
    let (stem, ext) = split_extension(trimmed);
    let ext = if ext.len() > 32 { "" } else { ext };
    let room = 255 - ext.len();
    let mut cut = room.min(stem.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{}", &stem[..cut], ext)
}

/// `name` with ` (n)` before the extension for `n` above one.
#[must_use]
pub fn numbered(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_owned();
    }
    let (stem, ext) = split_extension(name);
    format!("{stem} ({n}){ext}")
}

/// Splits `photo.tar.gz` into `photo.tar` and `.gz`; a name with no dot after
/// its first character has no extension.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// A content type from a file name's extension; the unknown type otherwise.
#[must_use]
pub fn mime_for(name: &str) -> &'static str {
    let ext = name
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "txt" | "md" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "m4a" => "audio/mp4",
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => FileDescriptor::OCTET_STREAM,
    }
}

/// The platform's download folder: `LIGATURE_DOWNLOAD_DIR` if set; on Linux
/// `XDG_DOWNLOAD_DIR` from `user-dirs.dirs`, else `~/Downloads`; on macOS
/// `~/Downloads`. `None` where no home directory is known.
#[must_use]
pub fn platform_download_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(DOWNLOAD_DIR_ENV)
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    let home = PathBuf::from(home);
    if cfg!(target_os = "linux") {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|c| !c.is_empty())
            .map_or_else(|| home.join(".config"), PathBuf::from);
        if let Ok(text) = std::fs::read_to_string(config.join("user-dirs.dirs"))
            && let Some(dir) = xdg_download_dir(&text, &home)
        {
            return Some(dir);
        }
    }
    Some(home.join("Downloads"))
}

/// Reads `XDG_DOWNLOAD_DIR="$HOME/..."` from a `user-dirs.dirs` file.
fn xdg_download_dir(text: &str, home: &Path) -> Option<PathBuf> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("XDG_DOWNLOAD_DIR=") else {
            continue;
        };
        let value = rest.trim().trim_matches('"');
        let value = value
            .strip_prefix("$HOME/")
            .map_or_else(|| PathBuf::from(value), |tail| home.join(tail));
        if value.as_os_str().is_empty() {
            return None;
        }
        return Some(value);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_rule_keeps_a_base_name_only() {
        assert_eq!(safe_name("photo.jpg"), "photo.jpg");
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("C:\\Users\\x\\a.txt"), "a.txt");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("..."), "file");
        assert_eq!(safe_name("a\u{0}b\n.txt"), "ab.txt");
        assert_eq!(safe_name("   "), "file");
        let long = format!("{}.tar.gz", "x".repeat(300));
        let cut = safe_name(&long);
        assert!(cut.len() <= 255);
        assert!(cut.ends_with(".gz"));
    }

    #[test]
    fn numbering_goes_before_the_extension() {
        assert_eq!(numbered("photo.jpg", 1), "photo.jpg");
        assert_eq!(numbered("photo.jpg", 2), "photo (2).jpg");
        assert_eq!(numbered("archive.tar.gz", 3), "archive.tar (3).gz");
        assert_eq!(numbered("README", 2), "README (2)");
    }

    #[test]
    fn a_landing_writes_a_part_file_then_renames_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileDescriptor::new("a", "notes.txt", 5);
        let mut landing = DirectoryStore::open_in(dir.path(), &file).unwrap();
        assert!(dir.path().join("notes.txt.ligature-part").exists());
        landing.write(b"hel").unwrap();
        landing.write(b"lo").unwrap();
        let landed = landing
            .finish(Timestamp::from_unix(1_700_000_000).ok())
            .unwrap();
        assert_eq!(landed.name, "notes.txt");
        assert_eq!(landed.size, 5);
        assert_eq!(
            std::fs::read(dir.path().join("notes.txt")).unwrap(),
            b"hello"
        );
        assert!(!dir.path().join("notes.txt.ligature-part").exists());
        let mtime = std::fs::metadata(dir.path().join("notes.txt"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            mtime
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            1_700_000_000
        );
        // The same name again lands as (2); an aborted one leaves nothing.
        let mut second = DirectoryStore::open_in(dir.path(), &file).unwrap();
        second.write(b"again").unwrap();
        let landed = second.finish(None).unwrap();
        assert_eq!(landed.name, "notes (2).txt");
        let third = DirectoryStore::open_in(dir.path(), &file).unwrap();
        assert!(dir.path().join("notes (3).txt.ligature-part").exists());
        third.abort();
        assert!(!dir.path().join("notes (3).txt.ligature-part").exists());
        assert!(!dir.path().join("notes (3).txt").exists());
    }

    #[test]
    fn a_memory_landing_returns_the_bytes_and_their_hash() {
        let file = FileDescriptor::new("a", "x", 3);
        let mut landing = MemoryLanding::open(&file);
        landing.write(b"abc").unwrap();
        let landed = landing.finish(None).unwrap();
        assert_eq!(landed.bytes.as_deref(), Some(&b"abc"[..]));
        assert_eq!(landed.sha256, "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0");
    }

    #[test]
    fn the_xdg_download_dir_is_read_from_user_dirs() {
        let home = Path::new("/home/x");
        let text =
            "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Down loads\"\n";
        assert_eq!(
            xdg_download_dir(text, home),
            Some(PathBuf::from("/home/x/Down loads"))
        );
        assert_eq!(
            xdg_download_dir("XDG_DOWNLOAD_DIR=\"/mnt/dl\"", home),
            Some(PathBuf::from("/mnt/dl"))
        );
        assert_eq!(
            xdg_download_dir("XDG_MUSIC_DIR=\"$HOME/Music\"", home),
            None
        );
    }
}
