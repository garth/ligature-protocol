//! A peer the daemon does not recognise reaches nothing but the pairing
//! offer: its gossip stream is reset whether or not a ceremony is running,
//! it is dropped at once outside a ceremony, and one kept for a ceremony is
//! dropped when the ceremony ends (LP-121).

use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::run::spawn_connection_events;
use lp_core::facts::Role;
use lp_core::frame::{GossipHave, NOT_ADMITTED_CODE, StreamHeader};
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
    until("kept for pairing while a ceremony runs", || {
        a.manager().peer(s_key.public()).is_some()
    })
    .await;
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

/// Alice's daemon, initialised from facts, on the real event loop.
fn alices_daemon(alice: &PersonKeys, name: &str, bind: &str) -> (Arc<Daemon>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (a, events) = Daemon::open(loc, bind.parse().unwrap(), Some(name)).unwrap();
    a.store()
        .insert(&facts::person_fact(alice, "Alice", 1, at(0)).unwrap())
        .unwrap();
    a.store()
        .insert(
            &facts::role_fact(alice, Role::Binding, *a.binding().public(), true, at(1)).unwrap(),
        )
        .unwrap();
    a.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
        .unwrap();
    spawn_connection_events(Arc::clone(&a), events);
    (a, dir)
}

/// A stranger: a bare manager with a key of its own.
fn stranger() -> (Manager, SigningKey) {
    let key = SigningKey::generate().unwrap();
    let ep = Endpoint::bind("127.0.0.1:0".parse().unwrap(), &key).unwrap();
    let (m, _events) = Manager::start(ep);
    (m, key)
}

/// A daemon with no identity keeps every connection for pairing; the moment
/// it is initialised, the strangers it kept are dropped (LP-121).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn strangers_are_swept_when_the_daemon_is_initialised() {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (a, events) = Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some("a")).unwrap();
    spawn_connection_events(Arc::clone(&a), events);
    let a_addr = a.manager().endpoint().local_addr().unwrap();
    let (s, s_key) = stranger();
    let _conn = s.connect(a_addr).await.unwrap();
    until("kept while uninitialised", || {
        a.manager().peer(s_key.public()).is_some()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(a.manager().peer(s_key.public()).is_some(), "still kept");

    a.init("Alice", None).unwrap();
    until("dropped once initialised", || {
        a.manager().peer(s_key.public()).is_none()
    })
    .await;
    a.shutdown();
}

/// While the daemon waits for an introducer, a stranger is kept; closing the
/// pairing window drops it (LP-121).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn strangers_are_swept_when_the_pairing_window_closes() {
    let alice = PersonKeys::generate().unwrap();
    let (a, _dir) = alices_daemon(&alice, "a", "127.0.0.1:0");
    let a_addr = a.manager().endpoint().local_addr().unwrap();
    let window = a.expect_pairing().unwrap();
    let (s, s_key) = stranger();
    let _conn = s.connect(a_addr).await.unwrap();
    until("kept while the window is open", || {
        a.manager().peer(s_key.public()).is_some()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(a.manager().peer(s_key.public()).is_some(), "still kept");

    a.stop_pairing();
    until("dropped when the window closes", || {
        a.manager().peer(s_key.public()).is_none()
    })
    .await;
    drop(window);
    a.shutdown();
}

/// The gossip handler checks trust itself: a peer the manager has admitted
/// (as a bug or a test might) but the store does not trust has its gossip
/// stream reset with code 2 and sees no `have` (LP-121).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gossip_handler_refuses_a_peer_the_store_does_not_trust() {
    let alice = PersonKeys::generate().unwrap();
    let (a, _dir) = alices_daemon(&alice, "a", "127.0.0.1:0");
    let a_addr = a.manager().endpoint().local_addr().unwrap();
    let ceremony = a.begin_ceremony();
    let (s, s_key) = stranger();
    let conn = s.connect(a_addr).await.unwrap();
    until("kept for the ceremony", || {
        a.manager().peer(s_key.public()).is_some()
    })
    .await;
    // Admitted at the manager, so the stream is dispatched to the handler.
    a.manager().admit(s_key.public());
    let mut gossip = open_on(&conn, StreamHeader::GOSSIP).await.unwrap();
    gossip.send(&GossipHave::new(vec![])).await.unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(5), gossip.recv::<GossipHave>())
        .await
        .expect("the handler answers, by resetting");
    match answer {
        Err(e) => {
            let text = e.to_string();
            assert!(
                text.contains(&format!("error {NOT_ADMITTED_CODE}")),
                "reset with the not-admitted code: {text}"
            );
        }
        other => panic!("a peer the store does not trust gets no have: {other:?}"),
    }
    assert!(
        a.manager().peer(s_key.public()).is_some(),
        "the ceremony still keeps it"
    );
    drop(ceremony);
    a.shutdown();
}

/// A stream the newcomer opens before the introducer has admitted it waits,
/// and is answered once the ceremony has made the newcomer one of ours
/// (ADR 0038 decision 3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_newcomers_stream_opened_before_admission_is_answered_after_the_ceremony() {
    use ligatured::ceremony::{Prompt, introduce, join};
    let alice = PersonKeys::generate().unwrap();
    // Every interface, so the offer's addresses are dialable.
    let (i, _di) = alices_daemon(&alice, "phone", "0.0.0.0:0");
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (n, events) = Daemon::open(loc, "0.0.0.0:0".parse().unwrap(), Some("laptop")).unwrap();
    spawn_connection_events(Arc::clone(&n), events);
    let i_key = *i.binding().public();

    let (n_code_tx, mut n_code_rx) = tokio::sync::mpsc::channel(1);
    let (n_confirm_tx, n_confirm_rx) = tokio::sync::oneshot::channel();
    let (offer_tx, mut offer_rx) = tokio::sync::mpsc::channel(1);
    let joining = {
        let n = Arc::clone(&n);
        tokio::spawn(async move {
            join(
                &n,
                Prompt {
                    code: n_code_tx,
                    confirm: n_confirm_rx,
                },
                offer_tx,
            )
            .await
        })
    };
    let offer = tokio::time::timeout(Duration::from_secs(5), offer_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let (i_code_tx, mut i_code_rx) = tokio::sync::mpsc::channel(1);
    let (i_confirm_tx, i_confirm_rx) = tokio::sync::oneshot::channel();
    let introducing = {
        let i = Arc::clone(&i);
        tokio::spawn(async move {
            introduce(
                &i,
                &offer,
                true,
                Prompt {
                    code: i_code_tx,
                    confirm: i_confirm_rx,
                },
            )
            .await
        })
    };

    // The introducer has dialled: the newcomer opens its gossip stream now,
    // before anyone has confirmed, and gets nothing yet.
    until("the introducer dialled the newcomer", || {
        n.manager().peer(&i_key).is_some()
    })
    .await;
    let mut gossip = n
        .manager()
        .open(&i_key, StreamHeader::GOSSIP)
        .await
        .unwrap();
    gossip.send(&GossipHave::new(vec![])).await.unwrap();
    let early = tokio::time::timeout(Duration::from_millis(500), gossip.recv::<GossipHave>()).await;
    assert!(
        early.is_err(),
        "held, not answered, before admission: {early:?}"
    );

    // Both sides see the same code and confirm.
    let n_code = tokio::time::timeout(Duration::from_secs(5), n_code_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let i_code = tokio::time::timeout(Duration::from_secs(5), i_code_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(n_code, i_code);
    n_confirm_tx.send(true).unwrap();
    i_confirm_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(10), introducing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), joining)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // The held stream is dispatched: the introducer answers with its have.
    let answer = tokio::time::timeout(Duration::from_secs(5), gossip.recv::<GossipHave>())
        .await
        .expect("answered in time")
        .expect("not reset");
    assert!(
        answer.is_some_and(|have| !have.facts.is_empty()),
        "the introducer's have lists its facts"
    );
    assert!(n.is_initialised());
    assert!(n.recognise(&i_key).unwrap().is_trusted());
    i.shutdown();
    n.shutdown();
}
