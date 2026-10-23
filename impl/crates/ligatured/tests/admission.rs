//! A peer the daemon does not recognise reaches nothing but the pairing
//! offer: its gossip stream is reset whether or not a ceremony is running,
//! it is dropped at once outside a ceremony, and one kept for a ceremony is
//! dropped when the ceremony ends (LP-121).

use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::run::spawn_connection_events;
use lp_core::facts::Role;
use lp_core::frame::{GossipHave, StreamHeader};
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_crypto::signing::SigningKey;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_transport::manager::{Manager, open_on};
use lp_transport::quic::Endpoint;

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
async fn a_stranger_gets_no_gossip_and_is_dropped_when_the_ceremony_ends() {
    // Alice's daemon, initialised and classifying its connections.
    let alice = PersonKeys::generate().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (a, events) = Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some("a")).unwrap();
    a.store()
        .insert(&facts::person_fact(&alice, "Alice", 1, at(0)).unwrap())
        .unwrap();
    a.store()
        .insert(
            &facts::role_fact(&alice, Role::Binding, *a.binding().public(), true, at(1)).unwrap(),
        )
        .unwrap();
    a.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
        .unwrap();
    let _events = spawn_connection_events(Arc::clone(&a), events);
    let a_addr = a.manager().endpoint().local_addr().unwrap();

    // A stranger with a key of its own.
    let s_key = SigningKey::generate().unwrap();
    let s_ep = Endpoint::bind("127.0.0.1:0".parse().unwrap(), &s_key).unwrap();
    let (s, _s_events) = Manager::start(s_ep);

    // Outside a ceremony: the gossip it opens in its first flight never gets
    // an answer, and the connection is dropped.
    let conn = s.connect(a_addr).await.unwrap();
    let mut gossip = open_on(&conn, StreamHeader::GOSSIP).await.unwrap();
    gossip.send(&GossipHave::new(vec![])).await.unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(5), gossip.recv::<GossipHave>())
        .await
        .unwrap();
    assert!(
        !matches!(answer, Ok(Some(_))),
        "a stranger must not receive have: {answer:?}"
    );
    until("the stranger is dropped", || {
        a.manager().peer(s_key.public()).is_none()
    })
    .await;

    // During a ceremony: the stranger is kept, still gets no gossip, and goes
    // when the ceremony ends.
    let ceremony = a.begin_ceremony();
    let conn = s.connect(a_addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        a.manager().peer(s_key.public()).is_some(),
        "kept for pairing while a ceremony runs"
    );
    let mut gossip = open_on(&conn, StreamHeader::GOSSIP).await.unwrap();
    gossip.send(&GossipHave::new(vec![])).await.unwrap();
    // The stream is held: no answer within a wait, and the stranger stays.
    let answer = tokio::time::timeout(Duration::from_secs(2), gossip.recv::<GossipHave>()).await;
    assert!(
        !matches!(answer, Ok(Ok(Some(_)))),
        "a stranger kept for pairing gets no gossip either: {answer:?}"
    );
    assert!(a.manager().peer(s_key.public()).is_some());
    drop(ceremony);
    until("the stranger is dropped when the ceremony ends", || {
        a.manager().peer(s_key.public()).is_none()
    })
    .await;
    // What it had waiting is reset with the connection.
    let answer = tokio::time::timeout(Duration::from_secs(5), gossip.recv::<GossipHave>())
        .await
        .unwrap();
    assert!(
        !matches!(answer, Ok(Some(_))),
        "held streams end without an answer: {answer:?}"
    );
    a.shutdown();
}
