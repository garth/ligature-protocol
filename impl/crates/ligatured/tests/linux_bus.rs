//! The Linux backend against the real session bus: a `notify-send` is seen
//! as a post, a copy is shown and closed. Needs a session bus with a
//! notification daemon, so it is ignored by default; run it by hand with
//! `cargo test -p ligatured --test linux_bus -- --ignored` (LP-91).
#![cfg(target_os = "linux")]

use std::time::Duration;

use ligatured::notifications::{Event, Notification, Notifications as _};
use ligatured::notifications_linux::LinuxNotifications;
use lp_core::wire::Timestamp;
use lp_crypto::signing::SigningKey;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a session bus with a notification daemon"]
async fn notify_send_is_seen_and_a_copy_is_shown_then_closed() {
    let icons = tempfile::tempdir().unwrap();
    let backend = LinuxNotifications::start(icons.path().join("icons")).expect("a session bus");
    let mut events = backend.subscribe();
    // The monitor takes a moment to be in place.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(backend.emits(), "monitoring the bus");

    let status = std::process::Command::new("notify-send")
        .args([
            "--app-name=lp-test",
            "Ligature test",
            "seen by the monitor?",
        ])
        .status()
        .expect("notify-send on PATH");
    assert!(status.success());
    let posted = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(Event::Posted(n)) = events.recv().await
                && n.app_id == "lp-test"
            {
                return n;
            }
        }
    })
    .await
    .expect("the notify-send was seen");
    assert_eq!(posted.title.as_deref(), Some("Ligature test"));
    assert_eq!(posted.body.as_deref(), Some("seen by the monitor?"));
    let bus_id: u32 = posted.id.parse().expect("bus id");
    assert!(bus_id > 0);

    // Show a copy on behalf of a made-up origin, then close it; neither is
    // seen as a post.
    let origin = *SigningKey::generate().unwrap().public();
    let copy = Notification {
        id: "7".to_owned(),
        app_id: "org.example.chat".to_owned(),
        app_name: "Chat".to_owned(),
        title: Some("Bob".to_owned()),
        body: Some("a copy from the phone".to_owned()),
        posted_at: Timestamp::now(),
        icon: None,
    };
    tokio::task::spawn_blocking({
        let backend = backend.clone();
        move || backend.show(&origin, "Pixel 8 Pro", &copy)
    })
    .await
    .unwrap()
    .expect("shown");
    tokio::time::sleep(Duration::from_millis(500)).await;
    tokio::task::spawn_blocking({
        let backend = backend.clone();
        move || backend.dismiss_copy(&origin, "7")
    })
    .await
    .unwrap()
    .expect("closed");
    let echoed = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            if let Ok(Event::Posted(n)) = events.recv().await
                && n.app_name.contains("Chat")
            {
                return n;
            }
        }
    })
    .await;
    assert!(echoed.is_err(), "our own copy was not seen as a post");
}
