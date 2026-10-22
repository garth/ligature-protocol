//! Received files land through the host, in MediaStore's Downloads.
//!
//! Android keeps the download folder from a library: a file there is a
//! MediaStore row the app inserts as pending, writes through a descriptor it
//! opened, and publishes when complete. The host does the row and the
//! descriptor and hands the node a path to write to (`/proc/self/fd/N` for
//! the descriptor it holds open); the node writes and hashes the bytes as on
//! the desktops, then asks the host to publish or discard the row. Sending
//! works the same way in the other direction: the app opens the shared
//! content and the node reads it as a path.

use std::io::Write as _;
use std::sync::Arc;

use ligatured::files::{FileStore, Landed, Landing};
use lp_core::Error;
use lp_core::files::FileDescriptor;
use lp_core::wire::{Bytes, Timestamp};
use lp_crypto::hash::Sha256Stream;

use crate::SharedHost;

/// The store that lands files through the host.
pub struct HostFiles {
    host: SharedHost,
}

impl std::fmt::Debug for HostFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostFiles")
    }
}

impl HostFiles {
    /// A store over `host`.
    #[must_use]
    pub fn new(host: SharedHost) -> Arc<Self> {
        Arc::new(Self { host })
    }
}

impl FileStore for HostFiles {
    fn available(&self) -> bool {
        true
    }

    fn open(&self, file: &FileDescriptor) -> Result<Box<dyn Landing>, Error> {
        let target = self
            .host
            .create_download(file.name.clone(), file.mime.clone(), file.size)
            .ok_or_else(|| Error::Io("the host could not create the download".to_owned()))?;
        let out = std::fs::OpenOptions::new()
            .write(true)
            .open(&target.path)
            .map_err(|e| {
                self.host.abort_download(target.id);
                Error::Io(format!("{}: {e}", target.path))
            })?;
        Ok(Box::new(HostLanding {
            host: self.host.clone(),
            id: target.id,
            file: Some(out),
            hasher: Sha256Stream::new(),
            written: 0,
            closed: false,
        }))
    }

    fn describe(&self) -> String {
        "Downloads".to_owned()
    }
}

/// One file being written through a descriptor the host holds.
struct HostLanding {
    host: SharedHost,
    id: u64,
    file: Option<std::fs::File>,
    hasher: Sha256Stream,
    written: u64,
    closed: bool,
}

impl Landing for HostLanding {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let Some(file) = self.file.as_mut() else {
            return Err(Error::Io("landing already closed".to_owned()));
        };
        file.write_all(bytes).map_err(|e| {
            if e.kind() == std::io::ErrorKind::StorageFull {
                Error::Io("no space".to_owned())
            } else {
                Error::Io(format!("download {}: {e}", self.id))
            }
        })?;
        self.hasher.update(bytes);
        self.written += bytes.len() as u64;
        Ok(())
    }

    fn digest(&self) -> String {
        Bytes(self.hasher.clone().finish().to_vec()).to_base64url()
    }

    fn finish(mut self: Box<Self>, _modified: Option<Timestamp>) -> Result<Landed, Error> {
        if let Some(mut file) = self.file.take() {
            file.flush()
                .map_err(|e| Error::Io(format!("download {}: {e}", self.id)))?;
        }
        self.closed = true;
        let landed = self
            .host
            .finish_download(self.id)
            .ok_or_else(|| Error::Io("the host could not publish the download".to_owned()))?;
        Ok(Landed {
            name: landed.name,
            location: Some(landed.location),
            path: None,
            bytes: None,
            sha256: Bytes(self.hasher.clone().finish().to_vec()).to_base64url(),
            size: self.written,
        })
    }

    fn abort(mut self: Box<Self>) {
        drop(self.file.take());
        self.closed = true;
        self.host.abort_download(self.id);
    }
}

impl Drop for HostLanding {
    fn drop(&mut self) {
        if !self.closed {
            drop(self.file.take());
            self.host.abort_download(self.id);
        }
    }
}
