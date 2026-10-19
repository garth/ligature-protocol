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

    // One connection per peer: dialling again returns the existing one, since
    // we dialled it ourselves.
    let again = a.connect(b_addr).await.unwrap();
    assert_eq!(again.raw().stable_id(), conn.raw().stable_id());
    assert_eq!(a.peers(), vec![*b_key.public()]);

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
