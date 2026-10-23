//! The connection manager on loopback: headers dispatch streams, unknown
//! purposes are refused without hurting the connection, one connection per
//! peer.

use std::sync::Arc;
use std::time::Duration;

use lp_core::frame::{GossipHave, StreamHeader};
use lp_crypto::signing::SigningKey;
use lp_transport::manager::{Event, Manager, open_on};
use lp_transport::quic::Endpoint;
use tokio::sync::mpsc;

fn endpoint() -> (Endpoint, SigningKey) {
    let key = SigningKey::generate().unwrap();
    let ep = Endpoint::bind("127.0.0.1:0".parse().unwrap(), &key).unwrap();
    (ep, key)
}

#[tokio::test]
async fn streams_are_dispatched_by_purpose_and_unknown_purposes_are_refused() {
    let (a_ep, _a_key) = endpoint();
    let (b_ep, b_key) = endpoint();
    let b_addr = b_ep.local_addr().unwrap();
    let (a, _a_events) = Manager::start(a_ep);
    let (b, mut b_events) = Manager::start(b_ep);

    let (got_tx, mut got_rx) = mpsc::channel::<GossipHave>(4);
    b.on_stream(
        StreamHeader::GOSSIP,
        Arc::new(move |_conn, mut stream| {
            let got_tx = got_tx.clone();
            Box::pin(async move {
                let msg: GossipHave = stream.recv().await.unwrap().unwrap();
                got_tx.send(msg).await.unwrap();
            })
        }),
    );

    let conn = a.connect(b_addr).await.unwrap();
    assert_eq!(conn.peer(), b_key.public());
    match tokio::time::timeout(Duration::from_secs(5), b_events.recv()).await {
        Ok(Some(Event::Connected(c))) => assert_eq!(c.peer(), a.endpoint().public()),
        other => panic!("expected Connected, got {other:?}"),
    }

    // An unknown purpose is refused: the stream is reset with code 1.
    let mut odd = open_on(&conn, "org.example.nothing").await.unwrap();
    odd.send(&GossipHave::new(vec![])).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(5), odd.recv::<GossipHave>())
        .await
        .unwrap();
    assert!(outcome.is_err(), "reset surfaces as an error: {outcome:?}");

    // The connection is unaffected: a gossip stream still works.
    let mut gossip = a.open(b_key.public(), StreamHeader::GOSSIP).await.unwrap();
    gossip.send(&GossipHave::new(vec![])).await.unwrap();
    gossip.finish();
    let got = tokio::time::timeout(Duration::from_secs(5), got_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(got.facts.is_empty());

    // One connection per peer: dialling again replaces the earlier one on
    // both sides (a completed dial proves the peer is there), still one peer.
    let again = a.connect(b_addr).await.unwrap();
    assert_ne!(again.raw().stable_id(), conn.raw().stable_id());
    assert_eq!(a.peers(), vec![*b_key.public()]);
    match tokio::time::timeout(Duration::from_secs(5), b_events.recv()).await {
        Ok(Some(Event::Connected(c))) => assert_eq!(c.peer(), a.endpoint().public()),
        other => panic!("expected Connected for the replacement, got {other:?}"),
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(1), conn.raw().closed())
            .await
            .is_ok(),
        "the earlier connection was closed"
    );
    let _conn = again;

    a.drop_peer(b_key.public());
    assert!(a.peer(b_key.public()).is_none());
    // b may first see the duplicate come and go; eventually it loses a.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, b_events.recv()).await {
            Ok(Some(Event::Disconnected(k)))
                if &k == a.endpoint().public() && b.peer(&k).is_none() =>
            {
                break;
            }
            Ok(Some(_)) => continue,
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }
    a.shutdown();
    b.shutdown();
}

/// A peer that restarts keeps its key but loses its connection; its first
/// dial after that must replace what we still hold, not be refused as a
/// duplicate (LP-98).
#[tokio::test]
async fn a_restarted_peer_replaces_the_connection_we_still_hold() {
    let (a_ep, _a_key) = endpoint();
    let (b_ep, b_key) = endpoint();
    let a_addr = a_ep.local_addr().unwrap();
    let (a, mut a_events) = Manager::start(a_ep);
    let (b, _b_events) = Manager::start(b_ep);

    // B dials A; A holds that connection.
    let first = b.connect(a_addr).await.unwrap();
    let held = match tokio::time::timeout(Duration::from_secs(5), a_events.recv()).await {
        Ok(Some(Event::Connected(c))) => c,
        other => panic!("expected Connected, got {other:?}"),
    };
    assert_eq!(held.peer(), b_key.public());

    // B "restarts": a new endpoint with the same key, the old one abandoned
    // without a close (its process is gone), and dials A again.
    std::mem::forget(first);
    std::mem::forget(b);
    let b2_ep = Endpoint::bind("127.0.0.1:0".parse().unwrap(), &b_key).unwrap();
    let (b2, _b2_events) = Manager::start(b2_ep);
    let second = b2.connect(a_addr).await.unwrap();
    assert_eq!(second.peer(), a.endpoint().public());

    // A now holds the new connection and told about it; a stream from A
    // reaches B2, not the abandoned endpoint.
    match tokio::time::timeout(Duration::from_secs(5), a_events.recv()).await {
        Ok(Some(Event::Connected(c))) => assert_eq!(c.peer(), b_key.public()),
        other => panic!("expected Connected for the redial, got {other:?}"),
    }
    let now_held = a.peer(b_key.public()).unwrap();
    assert_ne!(now_held.raw().stable_id(), held.raw().stable_id());
    let (got_tx, mut got_rx) = mpsc::channel::<GossipHave>(1);
    b2.on_stream(
        StreamHeader::GOSSIP,
        Arc::new(move |_conn, mut stream| {
            let got_tx = got_tx.clone();
            Box::pin(async move {
                let msg: GossipHave = stream.recv().await.unwrap().unwrap();
                got_tx.send(msg).await.unwrap();
            })
        }),
    );
    let mut gossip = a.open(b_key.public(), StreamHeader::GOSSIP).await.unwrap();
    gossip.send(&GossipHave::new(vec![])).await.unwrap();
    gossip.finish();
    tokio::time::timeout(Duration::from_secs(5), got_rx.recv())
        .await
        .expect("in time")
        .expect("reached the restarted peer");
    assert_eq!(a.peers(), vec![*b_key.public()]);
}

/// With admission required, a stream of an allowed purpose goes through at
/// once, any other waits for `admit`, and a peer kept for pairing only has
/// its streams held until it is dropped.
#[tokio::test]
async fn streams_wait_for_admission_and_are_refused_under_restriction() {
    let (a_ep, _a_key) = endpoint();
    let (b_ep, b_key) = endpoint();
    let b_addr = b_ep.local_addr().unwrap();
    let (a, _a_events) = Manager::start(a_ep);
    let (b, _b_events) = Manager::start(b_ep);
    b.require_admission(&["pairing"]);

    let (pairing_tx, mut pairing_rx) = mpsc::channel::<GossipHave>(4);
    b.on_stream(
        "pairing",
        Arc::new(move |_conn, mut stream| {
            let tx = pairing_tx.clone();
            Box::pin(async move {
                let msg: GossipHave = stream.recv().await.unwrap().unwrap();
                tx.send(msg).await.unwrap();
            })
        }),
    );
    let (gossip_tx, mut gossip_rx) = mpsc::channel::<GossipHave>(4);
    b.on_stream(
        StreamHeader::GOSSIP,
        Arc::new(move |_conn, mut stream| {
            let tx = gossip_tx.clone();
            Box::pin(async move {
                let msg: GossipHave = stream.recv().await.unwrap().unwrap();
                tx.send(msg).await.unwrap();
            })
        }),
    );

    let conn = a.connect(b_addr).await.unwrap();
    assert_eq!(conn.peer(), b_key.public());

    // The allowed purpose is dispatched before any verdict.
    let mut pairing = open_on(&conn, "pairing").await.unwrap();
    pairing.send(&GossipHave::new(vec![])).await.unwrap();
    pairing.finish();
    tokio::time::timeout(Duration::from_secs(5), pairing_rx.recv())
        .await
        .expect("pairing dispatched")
        .unwrap();

    // Gossip waits until the owner admits the peer.
    let mut gossip = open_on(&conn, StreamHeader::GOSSIP).await.unwrap();
    gossip.send(&GossipHave::new(vec![])).await.unwrap();
    gossip.finish();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), gossip_rx.recv())
            .await
            .is_err(),
        "held until admission"
    );
    b.admit(a.endpoint().public());
    tokio::time::timeout(Duration::from_secs(5), gossip_rx.recv())
        .await
        .expect("dispatched on admission")
        .unwrap();

    // A fresh connection kept for pairing only: its gossip waits, the
    // handler never runs, and dropping the peer resets what waited.
    let (c_ep, c_key) = endpoint();
    let (c, _c_events) = Manager::start(c_ep);
    let conn = c.connect(b_addr).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while b.peer(c_key.public()).is_none() {
        assert!(tokio::time::Instant::now() < deadline, "B accepted C");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    b.restrict(c_key.public());
    let mut held = open_on(&conn, StreamHeader::GOSSIP).await.unwrap();
    held.send(&GossipHave::new(vec![])).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), gossip_rx.recv())
            .await
            .is_err(),
        "the handler never ran"
    );
    b.drop_peer(c_key.public());
    let outcome = tokio::time::timeout(Duration::from_secs(5), held.recv::<GossipHave>())
        .await
        .unwrap();
    assert!(outcome.is_err(), "reset surfaces as an error: {outcome:?}");
}
