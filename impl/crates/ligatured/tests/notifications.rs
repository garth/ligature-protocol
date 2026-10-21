//! Three daemons of one person with fake notification backends: a post on one
//! is shown on the two others with its icon, a dismissal on a displaying
//! device reaches the origin and the third device in one hop, a copy is never
//! re-emitted, and a received dismiss is never forwarded (LP-91).

use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::notifications::{FakeNotifications, Icon, Notification};
use ligatured::run::spawn_connection_events;
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

async fn until(what: &str, check: impl Fn() -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(5), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notification_reaches_every_displaying_device_and_dismissals_fan_out() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let mut daemons = Vec::new();
    for name in ["phone", "linux", "mac"] {
        let dir = tempfile::tempdir().unwrap();
        let loc = Locations::resolve(Some(dir.path()), None);
        let (d, events) = Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some(name)).unwrap();
        daemons.push((d, events, dir));
    }
    let roles: Vec<_> = daemons
        .iter()
        .enumerate()
        .map(|(i, (d, _, _))| {
            facts::role_fact(
                &alice,
                Role::Binding,
                *d.binding().public(),
                true,
                at(1 + i as i64),
            )
            .unwrap()
        })
        .collect();
    for (d, _, _) in &daemons {
        d.store().insert(&person).unwrap();
        for r in &roles {
            d.store().insert(r).unwrap();
        }
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    let (mac, mac_events, _dm) = daemons.pop().unwrap();
    let (linux, linux_events, _dl) = daemons.pop().unwrap();
    let (phone, phone_events, _dp) = daemons.pop().unwrap();
    // The phone emits and displays, Linux emits and displays, the Mac only displays.
    let phone_n = FakeNotifications::new(true, true);
    let linux_n = FakeNotifications::new(true, true);
    let mac_n = FakeNotifications::new(false, true);
    let phone_sync = phone.attach_notifications(phone_n.clone());
    let linux_sync = linux.attach_notifications(linux_n.clone());
    let mac_sync = mac.attach_notifications(mac_n.clone());
    spawn_connection_events(phone.clone(), phone_events);
    spawn_connection_events(linux.clone(), linux_events);
    spawn_connection_events(mac.clone(), mac_events);

    // Everyone connects to everyone, and the manifests arrive.
    for (a, b) in [(&phone, &linux), (&phone, &mac), (&linux, &mac)] {
        a.manager()
            .connect(b.manager().endpoint().local_addr().unwrap())
            .await
            .unwrap();
    }
    until("manifests exchanged", || {
        [&phone, &linux, &mac].iter().all(|d| {
            d.registry()
                .peers_offering(lp_core::capability::NOTIFICATIONS_TYPE)
                .len()
                == 2
        })
    })
    .await;

    // An app on the phone posts, with an icon.
    let icon = Icon::png(vec![0x89, b'P', b'N', b'G', 1, 2, 3]);
    let posted = Notification {
        id: "42".to_owned(),
        app_id: "org.example.chat".to_owned(),
        app_name: "Chat".to_owned(),
        title: Some("Bob".to_owned()),
        body: Some("lunch?".to_owned()),
        posted_at: Timestamp::now(),
        icon: Some(icon.clone()),
    };
    phone_n.post(posted.clone());
    until("shown on linux and mac", || {
        linux_n.shown().len() == 1 && mac_n.shown().len() == 1
    })
    .await;
    let (origin, origin_name, shown) = linux_n.shown().remove(0);
    assert_eq!(origin, *phone.binding().public());
    // No device record is stored in this test, so the name falls back to the key.
    assert_eq!(origin_name, phone.binding().public().fingerprint());
    assert_eq!(shown, posted, "shown with its icon");
    assert_eq!(phone_sync.counters().posted, 2);
    // The copies are not re-emitted: nothing came back to the phone and the
    // Mac did not get it twice.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        phone_n.shown().is_empty(),
        "the phone shows no copy of its own"
    );
    assert_eq!(mac_n.shown().len(), 1);
    assert_eq!(linux_sync.counters().posted, 0);

    // A second post with the same id and no icon replaces in place; the icon
    // went once.
    let mut update = posted.clone();
    update.body = Some("lunch at one?".to_owned());
    phone_n.post(update.clone());
    until("updated on linux", || {
        linux_n
            .shown()
            .first()
            .is_some_and(|(_, _, n)| n.body == update.body)
    })
    .await;
    assert_eq!(linux_n.shown().len(), 1, "replaced, not added");

    // The user dismisses the copy on Linux: the phone cancels its own and the
    // Mac clears its copy, each told directly by Linux.
    linux_n.user_dismisses_copy(phone.binding().public(), "42");
    until("phone cancelled its own", || {
        phone_n.dismissed_own() == vec!["42".to_owned()]
    })
    .await;
    until("mac cleared its copy", || mac_n.shown().is_empty()).await;
    assert_eq!(linux_sync.counters().dismissed_out, 2);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        phone_sync.counters().dismissed_out,
        0,
        "a received dismiss is not forwarded"
    );
    assert_eq!(mac_sync.counters().dismissed_out, 0);

    // The Mac emits nothing: a post there goes nowhere.
    mac_n.post(posted.clone());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(mac_sync.counters().posted, 0);

    // Access revoked on the phone: its descriptor stops saying emits.
    phone_n.set_emits(false);
    phone_sync.advertise().await;
    until("linux sees the phone no longer emits", || {
        linux.registry().peers().iter().any(|p| {
            p.peer == *phone.binding().public()
                && p.capabilities.iter().any(|d| {
                    lp_core::capability::NotificationsParams::from_descriptor(d)
                        .is_some_and(|params| !params.emits && params.displays)
                })
        })
    })
    .await;
    for d in [&phone, &linux, &mac] {
        d.shutdown();
    }
}
