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

/// The same restart the other way round (LP-158): we dialled the peer, and
/// our key is the lower, so the crossed-dial tie-break would keep our dial.
/// The peer restarts without a close and dials us; our old connection still
/// looks alive until it times out, yet the peer's dial is not a crossing,
/// since ours is long established: the new one must replace it at once, or
/// every redial is dropped until the idle timeout.
#[tokio::test]
async fn a_restarted_peer_replaces_our_dial_to_it_even_when_our_key_is_lower() {
    let (mut a_ep, mut a_key) = endpoint();
    let (mut b_ep, mut b_key) = endpoint();
    if a_key.public() > b_key.public() {
        std::mem::swap(&mut a_ep, &mut b_ep);
        std::mem::swap(&mut a_key, &mut b_key);
    }
    let a_addr = a_ep.local_addr().unwrap();
    let b_addr = b_ep.local_addr().unwrap();
    let (a, mut a_events) = Manager::start(a_ep);
    let (b, _b_events) = Manager::start(b_ep);

    // A dials B; A holds its own dial.
    let ours = a.connect(b_addr).await.unwrap();
    match tokio::time::timeout(Duration::from_secs(5), a_events.recv()).await {
        Ok(Some(Event::Connected(c))) => assert_eq!(c.peer(), b_key.public()),
        other => panic!("expected Connected, got {other:?}"),
    }
    assert!(ours.dialled_by_us());
    // Long enough that a new dial cannot be one that crossed ours.
    tokio::time::sleep(Duration::from_secs(4)).await;

    // B restarts without a close and dials A.
    std::mem::forget(b);
    let b2_ep = Endpoint::bind("127.0.0.1:0".parse().unwrap(), &b_key).unwrap();
    let (b2, _b2_events) = Manager::start(b2_ep);
    let second = b2.connect(a_addr).await.unwrap();
    assert_eq!(second.peer(), a.endpoint().public());

    match tokio::time::timeout(Duration::from_secs(5), a_events.recv()).await {
        Ok(Some(Event::Connected(c))) => assert_eq!(c.peer(), b_key.public()),
        other => panic!("expected Connected for the restarted peer's dial, got {other:?}"),
    }
    let now_held = a.peer(b_key.public()).unwrap();
    assert!(
        !now_held.dialled_by_us(),
        "A holds the restarted peer's dial"
    );
    // And the connection works: a stream from A reaches B2.
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
    assert!(b2.peer(a.endpoint().public()).is_some(), "B2 kept its dial");
    drop(ours);
}

/// A manager that shuts down tells its peers: they see the connection go at
/// once instead of holding it for the idle timeout (60 s), so a restarted
/// daemon is redialled straight away (LP-158).
#[tokio::test]
async fn a_shutdown_reaches_the_peer_at_once() {
    let (a_ep, _a_key) = endpoint();
    let (b_ep, b_key) = endpoint();
    let a_addr = a_ep.local_addr().unwrap();
    let (a, mut a_events) = Manager::start(a_ep);
    let (b, _b_events) = Manager::start(b_ep);
    b.connect(a_addr).await.unwrap();
    match tokio::time::timeout(Duration::from_secs(5), a_events.recv()).await {
        Ok(Some(Event::Connected(c))) => assert_eq!(c.peer(), b_key.public()),
        other => panic!("expected Connected, got {other:?}"),
    }
    b.shutdown();
    tokio::time::timeout(Duration::from_secs(1), b.endpoint().wait_idle())
        .await
        .expect("the close went out");
    match tokio::time::timeout(Duration::from_secs(2), a_events.recv()).await {
        Ok(Some(Event::Disconnected(peer))) => assert_eq!(&peer, b_key.public()),
        other => panic!("expected Disconnected within 2 s, got {other:?}"),
    }
    assert!(a.peers().is_empty());
}

/// Both sides dial each other at once. Whichever dial lands first, the
/// connection dialled by the side with the lower key is the one both sides
/// keep (`we_keep_our_dial`), the other is closed, each side holds one
/// connection to the other, each was told about the kept one, and a stream
/// opened from either side reaches the other over it. The dials are started
/// together and awaited together; whether they cross on the wire or one
/// lands before the other completes depends on the machine, and the kept
/// connection is the same either way, since a completed dial of the lower
/// side replaces the higher side's and a completed dial of the higher side
/// is dropped in favour of the lower side's.
#[tokio::test]
async fn crossed_dials_keep_the_connection_dialled_by_the_lower_key() {
    let (a_ep, a_key) = endpoint();
    let (b_ep, b_key) = endpoint();
    let a_addr = a_ep.local_addr().unwrap();
    let b_addr = b_ep.local_addr().unwrap();
    let (a, mut a_events) = Manager::start(a_ep);
    let (b, mut b_events) = Manager::start(b_ep);
    let (a_got_tx, mut a_got_rx) = mpsc::channel::<GossipHave>(4);
    let (b_got_tx, mut b_got_rx) = mpsc::channel::<GossipHave>(4);
    for (m, tx) in [(&a, a_got_tx), (&b, b_got_tx)] {
        m.on_stream(
            StreamHeader::GOSSIP,
            Arc::new(move |_conn, mut stream| {
                let tx = tx.clone();
                Box::pin(async move {
                    let msg: GossipHave = stream.recv().await.unwrap().unwrap();
                    tx.send(msg).await.unwrap();
                })
            }),
        );
    }

    let (from_a, from_b) = tokio::join!(a.connect(b_addr), b.connect(a_addr));
    let from_a = from_a.unwrap();
    let from_b = from_b.unwrap();
    assert_eq!(from_a.peer(), b_key.public());
    assert_eq!(from_b.peer(), a_key.public());

    // The tie-break: the lower key's dial survives on both sides.
    let a_is_lower = a_key.public() < b_key.public();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let settled = |a: &Manager, b: &Manager| {
        a.peer(b_key.public())
            .is_some_and(|c| c.dialled_by_us() == a_is_lower)
            && b.peer(a_key.public())
                .is_some_and(|c| c.dialled_by_us() == !a_is_lower)
    };
    while !settled(&a, &b) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the crossed dials did not settle on the lower key's connection"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(a.peers(), vec![*b_key.public()], "one connection to b");
    assert_eq!(b.peers(), vec![*a_key.public()], "one connection to a");
    let a_holds = a.peer(b_key.public()).unwrap();
    let b_holds = b.peer(a_key.public()).unwrap();
    // The winner keeps the connection its `connect` returned. The loser's
    // `connect` returned either its own dial's connection, which is then
    // closed in favour of the winner's, or the winner's connection itself,
    // when that had already landed and the duplicate dial was dropped on
    // the spot; which one depends on the timing the runner gives the race.
    let (winner_holds, winner_from, loser_holds, loser_from) = if a_is_lower {
        (&a_holds, &from_a, &b_holds, &from_b)
    } else {
        (&b_holds, &from_b, &a_holds, &from_a)
    };
    assert_eq!(
        winner_holds.raw().stable_id(),
        winner_from.raw().stable_id()
    );
    if loser_from.raw().stable_id() != loser_holds.raw().stable_id() {
        assert!(
            tokio::time::timeout(Duration::from_secs(2), loser_from.raw().closed())
                .await
                .is_ok(),
            "the losing dial was closed"
        );
    }

    // Each side was told about the connection it keeps (possibly after the
    // one it dropped).
    let saw_kept = async |events: &mut mpsc::Receiver<Event>,
                          kept: &lp_transport::quic::Connection| {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Some(Event::Connected(c))) if c.raw().stable_id() == kept.raw().stable_id() => {
                    return true;
                }
                Ok(Some(_)) => continue,
                _ => return false,
            }
        }
    };
    assert!(
        saw_kept(&mut a_events, &a_holds).await,
        "a saw its kept connection"
    );
    assert!(
        saw_kept(&mut b_events, &b_holds).await,
        "b saw its kept connection"
    );

    // A stream from each side reaches the other over the kept connection.
    let mut to_b = a.open(b_key.public(), StreamHeader::GOSSIP).await.unwrap();
    to_b.send(&GossipHave::new(vec![])).await.unwrap();
    to_b.finish();
    tokio::time::timeout(Duration::from_secs(5), b_got_rx.recv())
        .await
        .expect("in time")
        .expect("reached b");
    let mut to_a = b.open(a_key.public(), StreamHeader::GOSSIP).await.unwrap();
    to_a.send(&GossipHave::new(vec![])).await.unwrap();
    to_a.finish();
    tokio::time::timeout(Duration::from_secs(5), a_got_rx.recv())
        .await
        .expect("in time")
        .expect("reached a");
    a.shutdown();
    b.shutdown();
}

/// The same tie-break when the dials do not cross on the wire but one
/// lands before the other is made: a later dial from the lower side
/// replaces the higher side's connection, and a later dial from the higher
/// side is dropped at once, its `connect` returning the connection already
/// held. Both orderings, deterministically.
#[tokio::test]
async fn a_dial_that_lands_after_the_peers_is_settled_by_the_same_tie_break() {
    let (x_ep, x_key) = endpoint();
    let (y_ep, y_key) = endpoint();
    // `lo` is the side with the lower key, `hi` the other.
    let ((lo, lo_key, lo_addr), (hi, hi_key, hi_addr)) = {
        let x = (x_ep.local_addr().unwrap(), x_ep);
        let y = (y_ep.local_addr().unwrap(), y_ep);
        let (xm, _) = Manager::start(x.1);
        let (ym, _) = Manager::start(y.1);
        if x_key.public() < y_key.public() {
            ((xm, x_key, x.0), (ym, y_key, y.0))
        } else {
            ((ym, y_key, y.0), (xm, x_key, x.0))
        }
    };

    // The higher side dials first and the dial lands; then the lower side
    // dials: its connection replaces the first on both sides.
    let first = hi.connect(lo_addr).await.unwrap();
    assert_eq!(first.peer(), lo_key.public());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while lo.peer(hi_key.public()).is_none() {
        assert!(tokio::time::Instant::now() < deadline, "lo holds hi's dial");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let second = lo.connect(hi_addr).await.unwrap();
    assert!(second.dialled_by_us());
    assert_eq!(
        lo.peer(hi_key.public()).unwrap().raw().stable_id(),
        second.raw().stable_id(),
        "the lower side keeps its own dial"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while hi.peer(lo_key.public()).is_none_or(|c| c.dialled_by_us()) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "hi adopts lo's dial"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(2), first.raw().closed())
            .await
            .is_ok(),
        "the higher side's dial is closed"
    );

    // Now the higher side dials again while the lower side's connection
    // lives: the dial is a duplicate, dropped on the spot, and `connect`
    // hands back the connection already held.
    let again = hi.connect(lo_addr).await.unwrap();
    assert!(!again.dialled_by_us());
    assert_eq!(
        again.raw().stable_id(),
        hi.peer(lo_key.public()).unwrap().raw().stable_id()
    );
    assert_eq!(hi.peers(), vec![*lo_key.public()]);
    assert_eq!(lo.peers(), vec![*hi_key.public()]);
    hi.shutdown();
    lo.shutdown();
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
