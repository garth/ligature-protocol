//! Where a daemon keeps its state and its control socket.
//!
//! One function resolves both directories so every binary agrees (milestone 1
//! decision LP-32). During milestone 1 the default data directory is `data/`
//! relative to the current working directory, which is the project root when
//! the daemon is run from a checkout; it is ignored by git. The runtime
//! directory, which holds the control socket, defaults to the data directory.
//! Platform locations (`$XDG_DATA_HOME/ligature/`, `~/Library/Application
//! Support/ligature/`, `$XDG_RUNTIME_DIR`, `$TMPDIR`) become the defaults
//! after milestone 1.

use std::path::{Path, PathBuf};

/// Name of the environment variable that sets the data directory.
pub const DATA_DIR_ENV: &str = "LIGATURE_DATA_DIR";
/// Name of the environment variable that sets the runtime directory.
pub const RUNTIME_DIR_ENV: &str = "LIGATURE_RUNTIME_DIR";
/// The milestone 1 default data directory, relative to the working directory.
pub const DEFAULT_DATA_DIR: &str = "data";
/// File name of the control socket inside the runtime directory.
pub const CONTROL_SOCKET: &str = "control.sock";
/// File name of the SQLite store inside the data directory.
pub const STORE_FILE: &str = "store.sqlite";
/// File name of the private key file inside the data directory.
pub const KEYS_FILE: &str = "keys.cbor";

/// The resolved directories of one daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locations {
    /// Holds the store and the key file.
    pub data_dir: PathBuf,
    /// Holds the control socket.
    pub runtime_dir: PathBuf,
}

impl Locations {
    /// Resolves the directories from explicit values, typically the command
    /// line with the environment already applied by the argument parser.
    /// `None` selects the milestone 1 default: `data/` for both.
    pub fn resolve(data_dir: Option<&Path>, runtime_dir: Option<&Path>) -> Self {
        let data_dir = data_dir.map_or_else(|| PathBuf::from(DEFAULT_DATA_DIR), Path::to_path_buf);
        let runtime_dir = runtime_dir.map_or_else(|| data_dir.clone(), Path::to_path_buf);
        Self {
            data_dir,
            runtime_dir,
        }
    }

    /// Path of the control socket.
    pub fn control_socket(&self) -> PathBuf {
        self.runtime_dir.join(CONTROL_SOCKET)
    }

    /// Path of the SQLite store.
    pub fn store(&self) -> PathBuf {
        self.data_dir.join(STORE_FILE)
    }

    /// Path of the private key file.
    pub fn keys(&self) -> PathBuf {
        self.data_dir.join(KEYS_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_data_for_both() {
        let l = Locations::resolve(None, None);
        assert_eq!(l.data_dir, PathBuf::from("data"));
        assert_eq!(l.runtime_dir, PathBuf::from("data"));
        assert_eq!(l.control_socket(), PathBuf::from("data/control.sock"));
        assert_eq!(l.store(), PathBuf::from("data/store.sqlite"));
        assert_eq!(l.keys(), PathBuf::from("data/keys.cbor"));
    }

    #[test]
    fn runtime_follows_data_unless_given() {
        let l = Locations::resolve(Some(Path::new("data/a")), None);
        assert_eq!(l.runtime_dir, PathBuf::from("data/a"));
        let l = Locations::resolve(Some(Path::new("data/a")), Some(Path::new("/run/x")));
        assert_eq!(l.runtime_dir, PathBuf::from("/run/x"));
    }
}
