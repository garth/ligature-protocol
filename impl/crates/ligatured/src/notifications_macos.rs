//! Notifications on macOS: display only, through `osascript`. Spec chapter
//! 05 section 3c and chapter 08 section 2b; milestone 3 (LP-93).
//!
//! A plain daemon has no bundle identifier, which the user notification
//! centre requires of a poster, so copies are shown with
//! `display notification` in AppleScript, which posts them as Script Editor's.
//! Such a notification cannot be updated or closed from outside, so a
//! repeated `post` shows again and a `dismiss` is dropped. A daemon may not
//! read other applications' notifications at all, so nothing is emitted. The
//! signed helper application of LP-94 lifts both limits.

use std::process::{Command, Stdio};
use std::sync::Arc;

use lp_core::{Error, PublicKey};
use tokio::sync::broadcast;

use crate::notifications::{Event, Notification, Notifications};

/// The macOS backend.
pub struct MacNotifications {
    events: broadcast::Sender<Event>,
}

impl std::fmt::Debug for MacNotifications {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacNotifications")
    }
}

impl MacNotifications {
    /// A backend that displays and never emits.
    #[must_use]
    pub fn new() -> Arc<Self> {
        let (events, _) = broadcast::channel(1);
        Arc::new(Self { events })
    }
}

/// A string as an AppleScript literal's content: backslashes and quotes
/// escaped, control characters dropped.
fn applescript(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() && c != '\n' => {}
            c => out.push(c),
        }
    }
    out
}

/// The script that shows `notification` from `origin_name`.
fn script(origin_name: &str, notification: &Notification) -> String {
    let heading = format!("{} · {}", notification.app_name, origin_name);
    let text = notification
        .body
        .as_deref()
        .or(notification.title.as_deref())
        .unwrap_or_default();
    let mut script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript(text),
        applescript(&heading)
    );
    if let (Some(title), Some(_)) = (&notification.title, &notification.body) {
        script.push_str(&format!(" subtitle \"{}\"", applescript(title)));
    }
    script
}

impl Notifications for MacNotifications {
    fn emits(&self) -> bool {
        false
    }

    fn displays(&self) -> bool {
        true
    }

    fn max_icon_bytes(&self) -> u64 {
        // `display notification` takes no image.
        0
    }

    fn show(
        &self,
        _origin: &PublicKey,
        origin_name: &str,
        notification: &Notification,
    ) -> Result<(), Error> {
        let script = script(origin_name, notification);
        // osascript takes a moment; a thread waits on it so the caller and
        // the runtime are not held and the child is reaped.
        std::thread::spawn(move || {
            let outcome = Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match outcome {
                Ok(status) if status.success() => {}
                Ok(status) => tracing::warn!(%status, "osascript could not show the notification"),
                Err(e) => tracing::warn!(%e, "osascript could not run"),
            }
        });
        Ok(())
    }

    fn dismiss_copy(&self, _origin: &PublicKey, _id: &str) -> Result<(), Error> {
        // Not possible without a bundle (LP-94); the copy stays until cleared.
        Ok(())
    }

    fn dismiss_own(&self, _id: &str) -> Result<(), Error> {
        // Nothing is emitted, so nothing is ever ours to cancel.
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lp_core::wire::Timestamp;

    #[test]
    fn the_script_quotes_and_names_the_origin() {
        let n = Notification {
            id: "1".to_owned(),
            app_id: "chat".to_owned(),
            app_name: "Chat".to_owned(),
            title: Some("Bob \"the\" builder".to_owned()),
            body: Some("back\\slash".to_owned()),
            posted_at: Timestamp::now(),
            icon: None,
        };
        assert_eq!(
            script("Pixel 8 Pro", &n),
            "display notification \"back\\\\slash\" with title \"Chat · Pixel 8 Pro\" subtitle \"Bob \\\"the\\\" builder\""
        );
        let only_title = Notification { body: None, ..n };
        assert_eq!(
            script("Pixel", &only_title),
            "display notification \"Bob \\\"the\\\" builder\" with title \"Chat · Pixel\""
        );
    }
}
