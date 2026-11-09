//! The permissions macOS asks the user for (LP-169): Accessibility to be
//! driven, Input Monitoring to drive others, Screen Recording to share the
//! screen, and the Camera and the Microphone to share a camera (LP-208).
//! Other platforms need none of them.
//!
//! macOS answers the preflight checks for a process from what was granted
//! when it started, and an Input Monitoring or Screen Recording grant takes
//! effect only in a new process. So while one was missing at start, the
//! daemon asks a short-lived copy of itself (`ligatured permissions`, which
//! prints [`Access::line`]) what a new process would see, and when one it
//! lacked has been granted it stops, for launchd's `KeepAlive` to start it
//! again with the grant in force. The Camera and the Microphone are asked
//! for in a dialog at the first share of a camera and apply to the running
//! process at once, so they are shown and never waited for.

use std::time::Duration;

/// One permission macOS asks the user for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Accessibility: posting events, so other devices can drive this Mac.
    Accessibility,
    /// Input Monitoring: watching the keyboard and mouse, so this Mac can
    /// drive others.
    InputMonitoring,
    /// Screen Recording: capturing the screen, so other devices can watch it.
    ScreenRecording,
    /// Camera: capturing a camera, so other devices can watch or use it.
    Camera,
    /// Microphone: a shared camera carries this device's microphone.
    Microphone,
}

impl Permission {
    /// Every permission, in the order they are shown.
    pub const ALL: [Self; 5] = [
        Self::Accessibility,
        Self::InputMonitoring,
        Self::ScreenRecording,
        Self::Camera,
        Self::Microphone,
    ];

    /// The permissions a grant of which takes effect only in a new process:
    /// what the daemon waits for and restarts on.
    pub const RESTARTING: [Self; 3] = [
        Self::Accessibility,
        Self::InputMonitoring,
        Self::ScreenRecording,
    ];

    /// Whether a grant of it takes effect only in a new process.
    #[must_use]
    pub fn restarts(self) -> bool {
        Self::RESTARTING.contains(&self)
    }

    /// The name on the command line and in [`Access::line`].
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Accessibility => "accessibility",
            Self::InputMonitoring => "input-monitoring",
            Self::ScreenRecording => "screen-recording",
            Self::Camera => "camera",
            Self::Microphone => "microphone",
        }
    }

    /// The name System Settings gives it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Accessibility => "Accessibility",
            Self::InputMonitoring => "Input Monitoring",
            Self::ScreenRecording => "Screen Recording",
            Self::Camera => "Camera",
            Self::Microphone => "Microphone",
        }
    }

    /// What it lets this device do.
    #[must_use]
    pub const fn enables(self) -> &'static str {
        match self {
            Self::Accessibility => "your other devices drive this one",
            Self::InputMonitoring => "this device's keyboard and mouse drive your others",
            Self::ScreenRecording => "your other devices watch this screen",
            Self::Camera => "your other devices watch and use this device's cameras",
            Self::Microphone => "a camera you share carries this device's microphone",
        }
    }

    /// The permission a name means, if any.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }
}

/// Which of the permissions a process holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Access {
    /// Accessibility.
    pub accessibility: bool,
    /// Input Monitoring.
    pub input_monitoring: bool,
    /// Screen Recording.
    pub screen_recording: bool,
    /// Camera.
    pub camera: bool,
    /// Microphone.
    pub microphone: bool,
}

impl Access {
    /// Whether `permission` is held.
    #[must_use]
    pub const fn holds(&self, permission: Permission) -> bool {
        match permission {
            Permission::Accessibility => self.accessibility,
            Permission::InputMonitoring => self.input_monitoring,
            Permission::ScreenRecording => self.screen_recording,
            Permission::Camera => self.camera,
            Permission::Microphone => self.microphone,
        }
    }

    /// Whether every permission that needs a new process is held, so there
    /// is nothing to wait for.
    #[must_use]
    pub fn complete(&self) -> bool {
        Permission::RESTARTING.into_iter().all(|p| self.holds(p))
    }

    /// One line naming each permission with 1 or 0, as `ligatured
    /// permissions` prints it.
    #[must_use]
    pub fn line(&self) -> String {
        Permission::ALL
            .into_iter()
            .map(|p| format!("{}={}", p.name(), u8::from(self.holds(p))))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Reads [`Access::line`]; `None` unless every permission that needs a
    /// new process is named. The Camera and the Microphone read as not held
    /// when the line leaves them out.
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        let mut seen = [None; 5];
        for pair in line.split_whitespace() {
            let (name, value) = pair.split_once('=')?;
            let held = match value {
                "1" => true,
                "0" => false,
                _ => return None,
            };
            if let Some(i) = Permission::ALL.iter().position(|p| p.name() == name) {
                seen[i] = Some(held);
            }
        }
        Some(Self {
            accessibility: seen[0]?,
            input_monitoring: seen[1]?,
            screen_recording: seen[2]?,
            camera: seen[3].unwrap_or(false),
            microphone: seen[4].unwrap_or(false),
        })
    }

    /// The permissions missing here and held in `now`: what a restart would
    /// bring into force. A withdrawal is not among them; the input capture
    /// already follows that live. Nor are the Camera and the Microphone,
    /// which need no restart.
    #[must_use]
    pub fn newly_granted(&self, now: &Self) -> Vec<Permission> {
        Permission::RESTARTING
            .into_iter()
            .filter(|&p| !self.holds(p) && now.holds(p))
            .collect()
    }
}

/// How long to wait between asking a new process: often at first, while
/// the user is likely in System Settings after the prompts, then rarely.
#[must_use]
pub fn poll_interval(since_start: Duration) -> Duration {
    if since_start < Duration::from_secs(10 * 60) {
        Duration::from_secs(3)
    } else {
        Duration::from_secs(30)
    }
}

/// What this process holds, as macOS answered it at start.
#[cfg(target_os = "macos")]
#[must_use]
pub fn at_start() -> Access {
    Access {
        accessibility: lp_macos::input::post_access(),
        input_monitoring: lp_macos::input::listen_access(),
        screen_recording: lp_macos::screen::screen_access(),
        camera: lp_macos::camera::camera_access() == lp_macos::camera::Access::Allowed,
        microphone: lp_macos::camera::microphone_access() == lp_macos::camera::Access::Allowed,
    }
}

/// What this process holds right now: Accessibility and Input Monitoring
/// follow System Settings live, as the Camera and the Microphone do; Screen
/// Recording is as at start, and a grant of it restarts the daemon.
#[cfg(target_os = "macos")]
#[must_use]
pub fn current() -> Access {
    Access {
        accessibility: lp_macos::input::accessibility_live(),
        input_monitoring: lp_macos::input::listen_access_live(),
        screen_recording: lp_macos::screen::screen_access(),
        camera: lp_macos::camera::camera_access() == lp_macos::camera::Access::Allowed,
        microphone: lp_macos::camera::microphone_access() == lp_macos::camera::Access::Allowed,
    }
}

/// Resolves with the permissions granted since start, once a new process
/// would hold one this one lacks; never when nothing was missing.
#[cfg(target_os = "macos")]
pub async fn granted_since_start() -> Vec<Permission> {
    let start = at_start();
    if start.complete() {
        return std::future::pending().await;
    }
    let Ok(exe) = std::env::current_exe() else {
        tracing::warn!("cannot find this program to check for new permissions");
        return std::future::pending().await;
    };
    let began = std::time::Instant::now();
    loop {
        tokio::time::sleep(poll_interval(began.elapsed())).await;
        let exe = exe.clone();
        let asked = tokio::task::spawn_blocking(move || {
            std::process::Command::new(exe).arg("permissions").output()
        })
        .await;
        let Ok(Ok(out)) = asked else { continue };
        let Some(now) = Access::parse(&String::from_utf8_lossy(&out.stdout)) else {
            continue;
        };
        let granted = start.newly_granted(&now);
        if !granted.is_empty() {
            return granted;
        }
    }
}

/// Asks macOS for `permission`, which lists Ligature under it in System
/// Settings and, for Accessibility, shows macOS's alert. A permission held
/// already is left as it is.
#[cfg(target_os = "macos")]
pub fn request(permission: Permission) {
    match permission {
        Permission::Accessibility => {
            lp_macos::input::prompt_accessibility();
        }
        Permission::InputMonitoring => lp_macos::input::request_listen_access(),
        Permission::ScreenRecording => {
            if !lp_macos::screen::screen_access() {
                lp_macos::screen::request_screen_access();
            }
        }
        // macOS's dialog the first time; nothing once it was answered.
        Permission::Camera => lp_macos::camera::request_camera_access(),
        Permission::Microphone => lp_macos::camera::request_microphone_access(),
    }
}

/// Nothing to ask for: only macOS has these permissions.
#[cfg(not(target_os = "macos"))]
pub fn request(_permission: Permission) {}

/// Never resolves: only macOS has permissions that need a new process.
#[cfg(not(target_os = "macos"))]
pub async fn granted_since_start() -> Vec<Permission> {
    std::future::pending().await
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Access = Access {
        accessibility: false,
        input_monitoring: false,
        screen_recording: false,
        camera: false,
        microphone: false,
    };

    #[test]
    fn a_line_reads_back_as_it_was_written() {
        let a = Access {
            accessibility: true,
            input_monitoring: false,
            screen_recording: true,
            camera: true,
            microphone: false,
        };
        assert_eq!(
            a.line(),
            "accessibility=1 input-monitoring=0 screen-recording=1 camera=1 microphone=0"
        );
        assert_eq!(Access::parse(&a.line()), Some(a));
        assert_eq!(Access::parse(&format!("{}\n", a.line())), Some(a));
    }

    #[test]
    fn a_line_missing_a_permission_or_garbled_reads_as_nothing() {
        assert_eq!(Access::parse("accessibility=1 input-monitoring=1"), None);
        assert_eq!(
            Access::parse("accessibility=yes input-monitoring=1 screen-recording=1"),
            None
        );
        assert_eq!(Access::parse(""), None);
        assert_eq!(Access::parse("error: no such command"), None);
    }

    #[test]
    fn a_name_the_line_does_not_know_is_passed_over() {
        let read =
            Access::parse("accessibility=1 input-monitoring=1 screen-recording=0 location=1");
        assert_eq!(
            read,
            Some(Access {
                accessibility: true,
                input_monitoring: true,
                screen_recording: false,
                ..NONE
            })
        );
        // The camera and the microphone are read when they are there.
        let read = Access::parse(
            "accessibility=0 input-monitoring=0 screen-recording=0 camera=1 microphone=1",
        );
        assert_eq!(
            read,
            Some(Access {
                camera: true,
                microphone: true,
                ..NONE
            })
        );
    }

    #[test]
    fn only_what_was_missing_and_is_now_held_restarts() {
        let start = Access {
            accessibility: true,
            ..NONE
        };
        let now = Access {
            input_monitoring: true,
            // Granted in macOS's dialog: in force already, no restart.
            camera: true,
            microphone: true,
            ..NONE
        };
        assert_eq!(start.newly_granted(&now), vec![Permission::InputMonitoring]);
        assert!(start.newly_granted(&start).is_empty());
        assert!(NONE.newly_granted(&NONE).is_empty());
    }

    #[test]
    fn nothing_is_waited_for_once_all_are_held() {
        // The camera and the microphone are not waited for.
        let all = Access {
            accessibility: true,
            input_monitoring: true,
            screen_recording: true,
            ..NONE
        };
        assert!(all.complete());
        assert!(!NONE.complete());
        assert!(
            !Access {
                camera: true,
                microphone: true,
                ..NONE
            }
            .complete()
        );
        assert!(Permission::ScreenRecording.restarts());
        assert!(!Permission::Camera.restarts() && !Permission::Microphone.restarts());
    }

    #[test]
    fn the_check_slows_down_after_ten_minutes() {
        assert_eq!(poll_interval(Duration::ZERO), Duration::from_secs(3));
        assert_eq!(
            poll_interval(Duration::from_secs(9 * 60)),
            Duration::from_secs(3)
        );
        assert_eq!(
            poll_interval(Duration::from_secs(11 * 60)),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn names_round_trip() {
        for p in Permission::ALL {
            assert_eq!(Permission::from_name(p.name()), Some(p));
        }
        assert_eq!(Permission::from_name("camera"), Some(Permission::Camera));
        assert_eq!(Permission::from_name("location"), None);
    }
}
