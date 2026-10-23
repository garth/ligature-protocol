//! Live capabilities: what A offers appears on B, changes follow, and a
//! disconnect withdraws everything.

use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use lp_core::capability::{CLIPBOARD_TYPE, Descriptor, Scope};
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_transport::manager::Event;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

async fn until(mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met in time"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn capabilities_appear_update_and_are_withdrawn() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let (a, mut a_events) = Daemon::open(
        Locations::resolve(Some(d1.path()), None),
        "127.0.0.1:0".parse().unwrap(),
        Some("a"),
    )
    .unwrap();
    a.manager().admit_all(); // this test drives the manager without the event loop (LP-121)
    let (b, mut b_events) = Daemon::open(
        Locations::resolve(Some(d2.path()), None),
        "127.0.0.1:0".parse().unwrap(),
        Some("b"),
    )
    .unwrap();
    b.manager().admit_all(); // this test drives the manager without the event loop (LP-121)
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

    // Drive both daemons' events the way `run` does.
    for (d, mut events) in [
        (
            a.clone(),
            std::mem::replace(&mut a_events, tokio::sync::mpsc::channel(1).1),
        ),
        (
            b.clone(),
            std::mem::replace(&mut b_events, tokio::sync::mpsc::channel(1).1),
        ),
    ] {
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

    a.registry()
        .offer(Descriptor::new(CLIPBOARD_TYPE, Scope::User))
        .await;
    let a_key = *a.binding().public();
    let b_key = *b.binding().public();
    let b_registry: Arc<_> = b.registry().clone();
    let a_registry: Arc<_> = a.registry().clone();

    a.manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    until(|| b_registry.peers_offering(CLIPBOARD_TYPE) == vec![a_key]).await;
    until(|| {
        a_registry
            .peers()
            .iter()
            .any(|p| p.peer == b_key && p.capabilities.is_empty())
    })
    .await;

    // A withdraws: B sees the update.
    a.registry().withdraw(CLIPBOARD_TYPE).await;
    until(|| {
        b_registry.peers_offering(CLIPBOARD_TYPE).is_empty()
            && b_registry.peers().iter().any(|p| p.peer == a_key)
    })
    .await;
    // B offers: A sees it.
    b.registry()
        .offer(Descriptor::new(CLIPBOARD_TYPE, Scope::User))
        .await;
    until(|| a_registry.peers_offering(CLIPBOARD_TYPE) == vec![b_key]).await;

    // A disconnects: everything from A is gone on B.
    a.manager().drop_peer(&b_key);
    until(|| b_registry.peers().is_empty()).await;
    a.shutdown();
    b.shutdown();
}
