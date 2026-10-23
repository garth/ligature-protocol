//! Two daemons of one person already connected (by address, as after a
//! ceremony) then discover each other over mDNS: the discovery loop
//! recognises the announced instance as the peer it holds and does not dial
//! it again, so the live connection is never replaced and nothing in flight
//! on it is lost (LP-117). Needs mDNS on the machine, as the Android
//! two-node test does.

use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::run::{spawn_connection_events, spawn_discovery};
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::trust::Recognition;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

async fn until(what: &str, check: impl Fn() -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_found_by_mdns_while_connected_keeps_its_connection() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let mut daemons = Vec::new();
    for name in ["a", "b"] {
        let dir = tempfile::tempdir().unwrap();
        let loc = Locations::resolve(Some(dir.path()), None);
        // Every interface, so the announced addresses are the ones dialled.
        let (d, events) = Daemon::open(loc, "0.0.0.0:0".parse().unwrap(), Some(name)).unwrap();
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
    spawn_connection_events(Arc::clone(&a), a_events);
    spawn_connection_events(Arc::clone(&b), b_events);

    // A reaches B by address first, as the ceremony does.
    let port = b.manager().endpoint().local_addr().unwrap().port();
    let ab = a
        .manager()
        .connect(std::net::SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .unwrap();
    assert_eq!(
        a.recognise(ab.peer()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    until("B holds A", || {
        b.manager().peer(a.binding().public()).is_some()
    })
    .await;
    let a_holds = ab.raw().stable_id();
    let b_holds = b
        .manager()
        .peer(a.binding().public())
        .unwrap()
        .raw()
        .stable_id();

    // Now both advertise and browse: each finds the other's instance.
    a.start_advertising().unwrap();
    b.start_advertising().unwrap();
    spawn_discovery(Arc::clone(&a));
    spawn_discovery(Arc::clone(&b));
    // Discovery dials within a few seconds; give it long enough to have
    // done so and then some.
    tokio::time::sleep(Duration::from_secs(6)).await;

    let same_a = a
        .manager()
        .peer(b.binding().public())
        .map(|c| c.raw().stable_id());
    let same_b = b
        .manager()
        .peer(a.binding().public())
        .map(|c| c.raw().stable_id());
    assert_eq!(same_a, Some(a_holds), "A's connection to B was replaced");
    assert_eq!(same_b, Some(b_holds), "B's connection to A was replaced");
    assert!(
        ab.raw().close_reason().is_none(),
        "the first connection still lives"
    );
    a.shutdown();
    b.shutdown();
}
