//! Two daemons of one person with fake clipboards: a copy on one appears on
//! the other and back, with no echo loop; an older set is ignored.

use std::sync::Arc;
use std::time::Duration;

use ligatured::clipboard::{Clipboard, FakeClipboard};
use ligatured::daemon::Daemon;
use lp_core::capability::{CLIPBOARD_TYPE, ClipboardSet};
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_transport::manager::Event;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

async fn until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn drive(d: Arc<Daemon>, mut events: tokio::sync::mpsc::Receiver<Event>) {
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            match ev {
                Event::Connected(conn) => {
                    if d.recognise(conn.peer())
                        .map(|r| r.is_trusted())
                        .unwrap_or(false)
                    {
                        let d = d.clone();
                        tokio::spawn(async move { d.on_trusted(&conn).await });
                    }
                }
                Event::Disconnected(peer) => d.on_disconnected(&peer),
            }
        }
    });
}

#[tokio::test]
async fn a_copy_on_one_device_appears_on_the_other_and_back() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let (a, ea) = Daemon::open(
        Locations::resolve(Some(d1.path()), None),
        "127.0.0.1:0".parse().unwrap(),
        Some("a"),
    )
    .unwrap();
    let (b, eb) = Daemon::open(
        Locations::resolve(Some(d2.path()), None),
        "127.0.0.1:0".parse().unwrap(),
        Some("b"),
    )
    .unwrap();
    for d in [&a, &b] {
        d.store().insert(&person).unwrap();
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    for (i, d) in [&a, &b].iter().enumerate() {
        let role = facts::role_fact(
            &alice,
            Role::Binding,
            *d.binding().public(),
            true,
            at(1 + i as i64),
        )
        .unwrap();
        a.store().insert(&role).unwrap();
        b.store().insert(&role).unwrap();
    }
    let clip_a = FakeClipboard::new();
    let clip_b = FakeClipboard::new();
    let sync_a = a.attach_clipboard(clip_a.clone());
    let sync_b = b.attach_clipboard(clip_b.clone());
    drive(a.clone(), ea);
    drive(b.clone(), eb);

    // Both advertise the clipboard once connected.
    a.manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    let a_key = *a.binding().public();
    let b_key = *b.binding().public();
    until("b sees a's clipboard", || {
        b.registry().peers_offering(CLIPBOARD_TYPE) == vec![a_key]
    })
    .await;
    until("a sees b's clipboard", || {
        a.registry().peers_offering(CLIPBOARD_TYPE) == vec![b_key]
    })
    .await;
    // Each asked the other for its value on connecting; let those empty
    // answers settle so the counters below see only the copies.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // A copies: B gets it.
    clip_a.user_copies("one");
    until("one on b", || {
        clip_b.read_text().unwrap().as_deref() == Some("one")
    })
    .await;
    // B copies: A gets it.
    clip_b.user_copies("two");
    until("two on a", || {
        clip_a.read_text().unwrap().as_deref() == Some("two")
    })
    .await;
    // Let any echoes play out; the counters must be exactly one each way.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(sync_a.counters(), (1, 1), "a: sent one, applied one");
    assert_eq!(sync_b.counters(), (1, 1), "b: sent one, applied one");

    // The same text copied again is not re-sent.
    clip_a.user_copies("two");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(sync_a.counters().0, 1);

    // An older set is ignored; a stranger's set is refused.
    let mut old = ClipboardSet::now("stale");
    old.set_at = at(0);
    assert!(!sync_b.apply(&a_key, &old).unwrap());
    let stranger = lp_crypto::signing::SigningKey::generate().unwrap();
    assert!(
        sync_b
            .apply(stranger.public(), &ClipboardSet::now("x"))
            .is_err()
    );
    assert_eq!(clip_b.read_text().unwrap().as_deref(), Some("two"));

    // Clipboard unavailable: the capability is withdrawn on the other side.
    clip_a.set_available(false);
    until("a's clipboard withdrawn on b", || {
        b.registry().peers_offering(CLIPBOARD_TYPE).is_empty()
    })
    .await;
    a.shutdown();
    b.shutdown();
}
