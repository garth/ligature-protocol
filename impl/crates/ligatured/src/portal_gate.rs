//! When the daemon may talk to the desktop portal (LP-157).
//!
//! The daemon starts with the user's systemd manager, before the graphical
//! session. Any call to `org.freedesktop.portal.Desktop` starts
//! `xdg-desktop-portal` by D-Bus activation, and a portal started then
//! inherits an environment without `XDG_CURRENT_DESKTOP` or
//! `WAYLAND_DISPLAY`: it settles on fallbacks for the whole login, and the
//! InputCapture and ScreenCast portals, for every application, are missing
//! until it is restarted. So the daemon waits until the manager's
//! environment names the session, which reading it from systemd does not
//! disturb, before it touches a portal.

use std::time::Duration;

/// How often the manager's environment is read while waiting.
const POLL: Duration = Duration::from_secs(2);

/// Whether a systemd manager environment (`NAME=value` entries) names a
/// graphical session: a desktop and a Wayland display, both non-empty.
#[must_use]
pub fn names_session<S: AsRef<str>>(environment: &[S]) -> bool {
    let set = |name: &str| {
        environment.iter().any(|e| {
            e.as_ref()
                .strip_prefix(name)
                .and_then(|rest| rest.strip_prefix('='))
                .is_some_and(|value| !value.is_empty())
        })
    };
    set("XDG_CURRENT_DESKTOP") && set("WAYLAND_DISPLAY")
}

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait SystemdManager {
    #[zbus(property)]
    fn environment(&self) -> zbus::Result<Vec<String>>;
}

/// The user manager's environment, or `None` where there is no systemd user
/// manager to ask.
async fn manager_environment() -> Option<Vec<String>> {
    let connection = zbus::Connection::session().await.ok()?;
    let manager = SystemdManagerProxy::new(&connection).await.ok()?;
    manager.environment().await.ok()
}

/// Waits until the graphical session has put its environment in the user
/// manager. Returns at once where there is no systemd user manager, since
/// nothing then activates the portal from a bare environment.
pub async fn wait_for_session() {
    let mut told = false;
    loop {
        match manager_environment().await {
            None => return,
            Some(env) if names_session(&env) => return,
            Some(_) => {
                if !told {
                    tracing::info!(
                        "waiting for the graphical session before using the desktop portal"
                    );
                    told = true;
                }
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::names_session;

    #[test]
    fn a_session_is_a_desktop_and_a_wayland_display() {
        let early = [
            "HOME=/home/a",
            "PATH=/usr/bin",
            "XDG_RUNTIME_DIR=/run/user/1000",
        ];
        assert!(!names_session(&early), "the manager before the session");
        let session = [
            "HOME=/home/a",
            "XDG_CURRENT_DESKTOP=Hyprland",
            "WAYLAND_DISPLAY=wayland-1",
        ];
        assert!(names_session(&session));
        assert!(!names_session(&["XDG_CURRENT_DESKTOP=Hyprland"]));
        assert!(!names_session(&[
            "XDG_CURRENT_DESKTOP=",
            "WAYLAND_DISPLAY=wayland-1"
        ]));
        assert!(
            !names_session(&["XDG_CURRENT_DESKTOP_X=1", "WAYLAND_DISPLAY=wayland-1"]),
            "a longer name is not the variable"
        );
    }
}
