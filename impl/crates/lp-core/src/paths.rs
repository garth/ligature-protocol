//! Where a daemon keeps its state and its control socket.
//!
//! One function resolves both directories so every binary agrees (milestone 1
//! decision LP-32). The defaults are the platform locations (LP-85): the data
//! directory is `$XDG_DATA_HOME/ligature` (`~/.local/share/ligature`) on Linux
//! and `~/Library/Application Support/ligature` on macOS, and the runtime
//! directory, which holds the control socket, is `$XDG_RUNTIME_DIR/ligature` on
//! Linux and `$TMPDIR/ligature` on macOS, each falling back to the data
//! directory when its variable is unset. An explicit data directory with no
//! explicit runtime directory keeps the socket beside the store, so two daemons
//! given two data directories never share a socket. Without a home directory
//! the milestone 1 default, `data/` under the working directory, remains.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Name of the environment variable that sets the data directory.
pub const DATA_DIR_ENV: &str = "LIGATURE_DATA_DIR";
/// Name of the environment variable that sets the runtime directory.
pub const RUNTIME_DIR_ENV: &str = "LIGATURE_RUNTIME_DIR";
/// The data directory used when no home directory is known, relative to the
/// working directory (the milestone 1 default).
pub const DEFAULT_DATA_DIR: &str = "data";
/// The directory name under the platform locations.
pub const DIR_NAME: &str = "ligature";
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

/// The platform whose locations apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Linux and anything else following the XDG base directories.
    Xdg,
    /// macOS.
    MacOs,
}

impl Platform {
    /// The platform this binary runs on.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Xdg
        }
    }
}

impl Locations {
    /// Resolves the directories from explicit values, typically the command
    /// line with the environment already applied by the argument parser, and
    /// the process environment for the platform defaults.
    pub fn resolve(data_dir: Option<&Path>, runtime_dir: Option<&Path>) -> Self {
        Self::resolve_with(data_dir, runtime_dir, Platform::current(), |name| {
            std::env::var_os(name)
        })
    }

    /// [`Self::resolve`] against a given platform and environment, so the
    /// rules can be tested without touching the process environment. `env`
    /// answers a variable's value as the environment would.
    pub fn resolve_with(
        data_dir: Option<&Path>,
        runtime_dir: Option<&Path>,
        platform: Platform,
        env: impl Fn(&str) -> Option<OsString>,
    ) -> Self {
        let nonempty = |name: &str| env(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        match (data_dir, runtime_dir) {
            (Some(data), Some(runtime)) => Self {
                data_dir: data.to_path_buf(),
                runtime_dir: runtime.to_path_buf(),
            },
            (Some(data), None) => Self {
                data_dir: data.to_path_buf(),
                runtime_dir: data.to_path_buf(),
            },
            (None, runtime) => {
                let home = nonempty("HOME");
                let data_dir = match (platform, home) {
                    (Platform::Xdg, Some(home)) => nonempty("XDG_DATA_HOME")
                        .unwrap_or_else(|| home.join(".local").join("share"))
                        .join(DIR_NAME),
                    (Platform::MacOs, Some(home)) => home
                        .join("Library")
                        .join("Application Support")
                        .join(DIR_NAME),
                    (_, None) => PathBuf::from(DEFAULT_DATA_DIR),
                };
                let runtime_dir = runtime.map(Path::to_path_buf).unwrap_or_else(|| {
                    let base = match platform {
                        Platform::Xdg => nonempty("XDG_RUNTIME_DIR"),
                        Platform::MacOs => nonempty("TMPDIR"),
                    };
                    base.map_or_else(|| data_dir.clone(), |b| b.join(DIR_NAME))
                });
                Self {
                    data_dir,
                    runtime_dir,
                }
            }
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

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn linux_defaults_follow_xdg() {
        let l = Locations::resolve_with(
            None,
            None,
            Platform::Xdg,
            env(&[("HOME", "/home/u"), ("XDG_RUNTIME_DIR", "/run/user/1000")]),
        );
        assert_eq!(l.data_dir, PathBuf::from("/home/u/.local/share/ligature"));
        assert_eq!(l.runtime_dir, PathBuf::from("/run/user/1000/ligature"));
        assert_eq!(
            l.control_socket(),
            PathBuf::from("/run/user/1000/ligature/control.sock")
        );
        assert_eq!(
            l.store(),
            PathBuf::from("/home/u/.local/share/ligature/store.sqlite")
        );
        assert_eq!(
            l.keys(),
            PathBuf::from("/home/u/.local/share/ligature/keys.cbor")
        );
    }

    #[test]
    fn xdg_data_home_overrides_the_default() {
        let l = Locations::resolve_with(
            None,
            None,
            Platform::Xdg,
            env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/data/u")]),
        );
        assert_eq!(l.data_dir, PathBuf::from("/data/u/ligature"));
        // No runtime directory in the environment: the socket sits beside the store.
        assert_eq!(l.runtime_dir, PathBuf::from("/data/u/ligature"));
    }

    #[test]
    fn macos_defaults() {
        let l = Locations::resolve_with(
            None,
            None,
            Platform::MacOs,
            env(&[("HOME", "/Users/u"), ("TMPDIR", "/var/folders/ab/T/")]),
        );
        assert_eq!(
            l.data_dir,
            PathBuf::from("/Users/u/Library/Application Support/ligature")
        );
        assert_eq!(l.runtime_dir, PathBuf::from("/var/folders/ab/T/ligature"));
    }

    #[test]
    fn without_a_home_the_working_directory_default_remains() {
        let l = Locations::resolve_with(None, None, Platform::Xdg, env(&[("HOME", "")]));
        assert_eq!(l.data_dir, PathBuf::from("data"));
        assert_eq!(l.runtime_dir, PathBuf::from("data"));
    }

    #[test]
    fn an_explicit_data_directory_keeps_the_socket_beside_the_store() {
        let l = Locations::resolve_with(
            Some(Path::new("data/a")),
            None,
            Platform::Xdg,
            env(&[("HOME", "/home/u"), ("XDG_RUNTIME_DIR", "/run/user/1000")]),
        );
        assert_eq!(l.data_dir, PathBuf::from("data/a"));
        assert_eq!(l.runtime_dir, PathBuf::from("data/a"));
    }

    #[test]
    fn an_explicit_runtime_directory_wins() {
        let l = Locations::resolve_with(
            Some(Path::new("data/a")),
            Some(Path::new("/run/x")),
            Platform::Xdg,
            env(&[("HOME", "/home/u")]),
        );
        assert_eq!(l.runtime_dir, PathBuf::from("/run/x"));
        let l = Locations::resolve_with(
            None,
            Some(Path::new("/run/y")),
            Platform::Xdg,
            env(&[("HOME", "/home/u"), ("XDG_RUNTIME_DIR", "/run/user/1000")]),
        );
        assert_eq!(l.data_dir, PathBuf::from("/home/u/.local/share/ligature"));
        assert_eq!(l.runtime_dir, PathBuf::from("/run/y"));
    }
}
