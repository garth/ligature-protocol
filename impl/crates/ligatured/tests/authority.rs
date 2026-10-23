//! Another person's device is trusted enough to connect and exchange
//! manifests, and gets nothing of the person's own: no clipboard, no
//! notifications, and a `set` it sends is refused (chapter 10; the
//! `same_person` checks of every synchroniser).

use std::sync::Arc;
use std::time::Duration;

use ligatured::clipboard::{Clipboard as _, FakeClipboard};
use ligatured::daemon::Daemon;
use ligatured::notifications::{FakeNotifications, Notification};
use ligatured::run::spawn_connection_events;
use lp_core::capability::{CLIPBOARD_TYPE, ClipboardMessage, ClipboardSet};
use lp_core::facts::Role;
use lp_core::frame::REFUSED_CODE;
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::trust::Recognition;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

async fn until(what: &str, check: impl Fn() -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(15), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what}");
}

struct Device {
    daemon: Arc<Daemon>,
    clipboard: Arc<FakeClipboard>,
    sync: Arc<ligatured::clipboard_sync::ClipboardSync>,
    notifications: Arc<FakeNotifications>,
    notification_sync: Arc<ligatured::notification_sync::NotificationSync>,
    _dir: tempfile::TempDir,
}

/// `owner`'s device, on the real event loop, knowing `other` as a person
/// whose device `other_binding` is.
fn device(owner: &PersonKeys, name: &str) -> Device {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (daemon, events) = Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some(name)).unwrap();
    daemon
        .update_keys(|k| k.set_joined(*owner.person(), Some(&owner.self_signing)))
        .unwrap();
    let clipboard = FakeClipboard::new();
    let sync = daemon.attach_clipboard(clipboard.clone());
    let notifications = FakeNotifications::new(true, true);
    let notification_sync = daemon.attach_notifications(notifications.clone());
    spawn_connection_events(Arc::clone(&daemon), events);
    Device {
        daemon,
        clipboard,
        sync,
        notifications,
        notification_sync,
        _dir: dir,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_persons_device_gets_nothing_and_its_set_is_refused() {
    let alice = PersonKeys::generate().unwrap();
    let bob = PersonKeys::generate().unwrap();
    let a = device(&alice, "alice-laptop");
    let b = device(&bob, "bob-phone");
    let a_key = *a.daemon.binding().public();
    let b_key = *b.daemon.binding().public();
    // Each store: both person facts, each device's binding role signed by
    // its own person, and a `person` role by the owner over the other.
    let shared = [
        facts::person_fact(&alice, "Alice", 1, at(0)).unwrap(),
        facts::person_fact(&bob, "Bob", 1, at(0)).unwrap(),
        facts::role_fact(&alice, Role::Binding, a_key, true, at(1)).unwrap(),
        facts::role_fact(&bob, Role::Binding, b_key, true, at(1)).unwrap(),
    ];
    for env in &shared {
        a.daemon.store().insert(env).unwrap();
        b.daemon.store().insert(env).unwrap();
    }
    a.daemon
        .store()
        .insert(&facts::role_fact(&alice, Role::Person, *bob.person(), false, at(2)).unwrap())
        .unwrap();
    b.daemon
        .store()
        .insert(&facts::role_fact(&bob, Role::Person, *alice.person(), false, at(2)).unwrap())
        .unwrap();
    assert!(matches!(
        a.daemon.recognise(&b_key).unwrap(),
        Recognition::KnownPersonBinding { .. }
    ));

    // Bob's phone connects; manifests cross, since nothing filters them yet.
    b.daemon
        .manager()
        .connect(a.daemon.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    until("manifests exchanged", || {
        a.daemon.registry().peers_offering(CLIPBOARD_TYPE) == vec![b_key]
            && b.daemon.registry().peers_offering(CLIPBOARD_TYPE) == vec![a_key]
    })
    .await;

    // Alice copies and an app of hers posts: Bob's phone sees neither.
    a.clipboard.user_copies("alice's secret");
    a.notifications.post(Notification {
        id: "1".to_owned(),
        app_id: "org.example.chat".to_owned(),
        app_name: "Chat".to_owned(),
        title: Some("private".to_owned()),
        body: Some("for Alice's eyes".to_owned()),
        posted_at: Timestamp::now(),
        icon: None,
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        b.clipboard.read_text().unwrap(),
        None,
        "no clipboard reached Bob"
    );
    assert_eq!(a.sync.counters().0, 0, "nothing was sent");
    assert!(
        b.notifications.shown().is_empty(),
        "no notification reached Bob"
    );
    assert_eq!(a.notification_sync.counters().posted, 0);
    assert!(
        !a.sync.request(&b_key).await,
        "Alice's device holds no clipboard stream to Bob's"
    );

    // Bob's phone pushes a set anyway: refused with code 3, not applied.
    let mut stream = b
        .daemon
        .manager()
        .open(&a_key, CLIPBOARD_TYPE)
        .await
        .unwrap();
    stream.send(&ClipboardSet::now("from bob")).await.unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(5), stream.recv::<ClipboardMessage>())
        .await
        .expect("answered in time");
    match answer {
        Err(e) => {
            let text = e.to_string();
            assert!(
                text.contains(&format!("error {REFUSED_CODE}")),
                "reset with the refused code: {text}"
            );
        }
        other => panic!("the set must be refused: {other:?}"),
    }
    assert_eq!(
        a.clipboard.read_text().unwrap().as_deref(),
        Some("alice's secret"),
        "Alice's clipboard is untouched"
    );
    assert!(
        a.daemon.manager().peer(&b_key).is_some(),
        "the session itself stays: Bob is known"
    );
    a.daemon.shutdown();
    b.daemon.shutdown();
}
