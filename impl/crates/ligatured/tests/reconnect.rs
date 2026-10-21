//! Two daemons of one person lose their session; the one that remembers where
//! the other was redials it and is back without any mDNS event (LP-83).

use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::run::{spawn_connection_events, spawn_reconnect};
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::trust::Recognition;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

#[tokio::test]
async fn a_peer_that_went_away_is_redialled_at_its_last_address() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let mut daemons = Vec::new();
    for name in ["a", "b"] {
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
    let (b, b_events, _db) = daemons.pop().unwrap();
    let (a, a_events, _da) = daemons.pop().unwrap();
    // Both handle their connection events; only A runs the reconnect loop.
    spawn_connection_events(a.clone(), a_events);
    spawn_connection_events(b.clone(), b_events);
    spawn_reconnect(a.clone(), Duration::from_millis(300));

    let b_addr = b.manager().endpoint().local_addr().unwrap();
    let ab = a.manager().connect(b_addr).await.unwrap();
    assert_eq!(
        a.recognise(ab.peer()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    // A's event loop has noted where B was.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(a.last_addrs(), vec![(*b.binding().public(), b_addr)]);

    // B drops A: A sees the session end.
    b.manager().drop_peer(a.binding().public());
    let gone = tokio::time::timeout(Duration::from_secs(5), async {
        while a.manager().peer(b.binding().public()).is_some() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "A noticed the loss");

    // Without any mDNS, A is back on the next tick of its reconnect loop.
    let back = tokio::time::timeout(Duration::from_secs(5), async {
        while a.manager().peer(b.binding().public()).is_none()
            || b.manager().peer(a.binding().public()).is_none()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(back.is_ok(), "A redialled B's last address");

    // A re-query is asked for only while a binding is missing: B is back, so
    // calling it by hand is harmless and the browser restarts without one.
    a.requery();
    for d in [&a, &b] {
        d.shutdown();
    }
}
