//! Three daemons of one person: A revokes C; B learns it from A; C can no
//! longer be recognised by B, whether B hears by push or on the next session.

use std::sync::Arc;
use std::time::Duration;

use ligatured::control::{Client, Empty, Request, serve, types};
use ligatured::daemon::Daemon;
use ligatured::gossip::run_dialer;
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::trust::Recognition;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

fn daemon(name: &str) -> (Arc<Daemon>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (d, _events) = Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some(name)).unwrap();
    (d, dir)
}

#[tokio::test]
async fn a_revocation_made_on_one_device_reaches_the_others() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let (a, da) = daemon("a");
    let (b, _db) = daemon("b");
    let (c, _dc) = daemon("c");
    let roles: Vec<_> = [&a, &b, &c]
        .iter()
        .enumerate()
        .map(|(i, d)| {
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
    for d in [&a, &b, &c] {
        d.store().insert(&person).unwrap();
        for r in &roles {
            d.store().insert(r).unwrap();
        }
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    assert_eq!(
        b.recognise(c.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );

    // A is connected to B; A revokes C through its control channel.
    let loc_a = Locations::resolve(Some(da.path()), None);
    let _server = serve(a.clone(), &loc_a.control_socket()).unwrap();
    let mut ctl = Client::connect(&loc_a.control_socket()).await.unwrap();
    let ab = a
        .manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    assert_eq!(
        a.recognise(ab.peer()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    let mut revoke = Request::new(types::REVOKE, 0);
    revoke.key = Some(*c.binding().public());
    let _: Empty = ctl.request(revoke).await.unwrap();
    assert_eq!(
        a.recognise(c.binding().public()).unwrap(),
        Recognition::Revoked
    );

    // B heard by push over the live connection.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        b.recognise(c.binding().public()).unwrap(),
        Recognition::Revoked,
        "pushed to B"
    );

    // C connects to B: authenticated, but B refuses it.
    let cb = c
        .manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    assert_eq!(cb.peer(), b.binding().public());
    // B's side sees C's key on its Connected event and refuses it.
    assert_eq!(
        b.recognise(c.binding().public()).unwrap(),
        Recognition::Revoked
    );
    b.manager().drop_peer(c.binding().public());
    assert!(b.manager().peer(c.binding().public()).is_none());

    // A fourth device that was offline learns on its next session with anyone.
    let (d, _dd) = daemon("d");
    d.store().insert(&person).unwrap();
    for r in &roles {
        d.store().insert(r).unwrap();
    }
    d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
        .unwrap();
    assert_eq!(
        d.recognise(c.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    let db_conn = d
        .manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    let result = run_dialer(d.store(), &db_conn).await.unwrap();
    assert_eq!(result.stored, 1, "the revocation");
    assert_eq!(
        d.recognise(c.binding().public()).unwrap(),
        Recognition::Revoked
    );

    // Revocation is not undone by gossip of the old role facts.
    assert!(a.store().about(c.binding().public()).unwrap().len() >= 2);
    for x in [&a, &b, &c, &d] {
        x.shutdown();
    }
}
