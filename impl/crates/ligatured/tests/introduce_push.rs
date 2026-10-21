//! Three daemons: A and B hold Alice's keys and are connected; A introduces a
//! new device C. B must learn C's facts at once over the live connection, not
//! on its next session (LP-80, found with a phone, a Linux machine and a Mac).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ligatured::ceremony::{Prompt, introduce, join};
use ligatured::daemon::Daemon;
use ligatured::run::default_bind;
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::trust::Recognition;
use tokio::sync::{mpsc, oneshot};

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

fn daemon(name: &str, bind: SocketAddr) -> (Arc<Daemon>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (d, _events) = Daemon::open(loc, bind, Some(name)).unwrap();
    (d, dir)
}

#[tokio::test]
async fn a_peer_already_connected_learns_the_newcomer_at_once() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let loopback: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (a, _da) = daemon("a", loopback);
    let (b, _db) = daemon("b", loopback);
    // C's offer lists its interface addresses, so it listens on all of them.
    let (c, _dc) = daemon("c", default_bind());
    for (i, d) in [&a, &b].iter().enumerate() {
        let role = facts::role_fact(
            &alice,
            Role::Binding,
            *d.binding().public(),
            true,
            at(1 + i as i64),
        )
        .unwrap();
        for x in [&a, &b] {
            x.store().insert(&role).unwrap();
        }
    }
    for d in [&a, &b] {
        d.store().insert(&person).unwrap();
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    assert!(!c.is_initialised());

    // A and B are in session before C appears.
    let ab = a
        .manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    assert_eq!(
        a.recognise(ab.peer()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    assert_eq!(
        b.recognise(c.binding().public()).unwrap(),
        Recognition::Stranger
    );

    // C shows its offer; A introduces it; both confirm the same code.
    let (offer_tx, mut offer_rx) = mpsc::channel(1);
    let (c_code_tx, mut c_code_rx) = mpsc::channel(1);
    let (c_confirm_tx, c_confirm_rx) = oneshot::channel();
    let c2 = c.clone();
    let joining = tokio::spawn(async move {
        let prompt = Prompt {
            code: c_code_tx,
            confirm: c_confirm_rx,
        };
        join(&c2, prompt, offer_tx).await
    });
    let offer = tokio::time::timeout(Duration::from_secs(5), offer_rx.recv())
        .await
        .expect("offer in time")
        .expect("offer shown");
    let (a_code_tx, mut a_code_rx) = mpsc::channel(1);
    let (a_confirm_tx, a_confirm_rx) = oneshot::channel();
    let a2 = a.clone();
    let introducing = tokio::spawn(async move {
        let prompt = Prompt {
            code: a_code_tx,
            confirm: a_confirm_rx,
        };
        introduce(&a2, &offer, false, prompt).await
    });
    let a_code = tokio::time::timeout(Duration::from_secs(5), a_code_rx.recv())
        .await
        .expect("code in time")
        .expect("code");
    let c_code = c_code_rx.recv().await.expect("code");
    assert_eq!(a_code, c_code);
    a_confirm_tx.send(true).unwrap();
    c_confirm_tx.send(true).unwrap();
    introducing.await.unwrap().expect("introduced");
    joining.await.unwrap().expect("joined");

    // Both ends of the ceremony know each other.
    assert_eq!(
        a.recognise(c.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: false }
    );
    assert_eq!(
        c.recognise(a.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    // C learnt B from the facts the introducer sent.
    assert_eq!(
        c.recognise(b.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    // B, connected to A throughout, learnt C by push without a new session.
    assert_eq!(
        b.recognise(c.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: false },
        "pushed to B"
    );
    for x in [&a, &b, &c] {
        x.shutdown();
    }
}
