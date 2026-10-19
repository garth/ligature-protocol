//! The ceremony refuses: a tampered offer, a device presenting a key other
//! than the one scanned, a connection lost mid-ceremony, a grant replayed to
//! a fresh newcomer. Nothing is stored in any of these.

use std::sync::Arc;
use std::time::Duration;

use ligatured::ceremony::{Prompt, run_introducer, run_newcomer};
use lp_core::wire::Timestamp;
use lp_crypto::signing::SigningKey;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::pairing::{
    Grant, Introducer, Newcomer, NewcomerState, Offer, PURPOSE, grant_roles, types,
};
use lp_transport::manager::{Manager, open_on};
use lp_transport::quic::{Connection, Endpoint};
use lp_transport::stream::FramedStream;
use tokio::sync::{mpsc, oneshot};

fn now() -> Timestamp {
    Timestamp::from_unix(1_760_000_000).unwrap()
}

struct Side {
    manager: Manager,
    streams: mpsc::Receiver<(Connection, FramedStream)>,
}

fn side(key: &SigningKey) -> Side {
    let ep = Endpoint::bind("127.0.0.1:0".parse().unwrap(), key).unwrap();
    let (manager, _events) = Manager::start(ep);
    let (tx, streams) = mpsc::channel(4);
    manager.on_stream(
        PURPOSE,
        Arc::new(move |conn, stream| {
            let tx = tx.clone();
            Box::pin(async move {
                let _ = tx.send((conn, stream)).await;
            })
        }),
    );
    Side { manager, streams }
}

fn prompt(answer: bool) -> Prompt {
    let (code_tx, _code_rx) = mpsc::channel(1);
    let (confirm_tx, confirm_rx) = oneshot::channel();
    let _ = confirm_tx.send(answer);
    Prompt {
        code: code_tx,
        confirm: confirm_rx,
    }
}

/// The introducer scanned an offer whose binding key was altered: the device
/// it reaches authenticates with a different key, and nothing proceeds.
#[tokio::test]
async fn a_tampered_offer_means_the_wrong_key_and_is_refused() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, now()).unwrap();
    let phone_key = SigningKey::generate().unwrap();
    let phone = side(&phone_key);

    let laptop_binding = SigningKey::generate().unwrap();
    let mut laptop = side(&laptop_binding);
    let newcomer = Newcomer::new(
        laptop_binding.clone(),
        SigningKey::generate().unwrap(),
        "laptop",
        "linux",
        now(),
    )
    .unwrap();
    let mut offer = newcomer.offer(vec![laptop.manager.endpoint().local_addr().unwrap()]);
    // Tampered: the scanned binding key is someone else's.
    offer.binding = *SigningKey::generate().unwrap().public();

    let newcomer_task = tokio::spawn(async move {
        // The introducer aborts at once and closes the connection, so the
        // stream may never reach the newcomer; if it does, it must still not
        // join.
        if let Ok(Some((conn, stream))) =
            tokio::time::timeout(Duration::from_secs(1), laptop.streams.recv()).await
        {
            let r = run_newcomer(&conn, stream, newcomer, prompt(true)).await;
            assert!(r.is_err(), "newcomer must not join: {r:?}");
        }
    });

    let conn = phone.manager.connect(offer.addrs[0]).await.unwrap();
    let stream = open_on(&conn, PURPOSE).await.unwrap();
    let introducer = Introducer::new(
        alice.self_signing.clone(),
        *alice.person(),
        person,
        *phone_key.public(),
        offer,
        true,
        vec![],
    );
    let r = run_introducer(&conn, stream, introducer, prompt(true), now()).await;
    assert!(
        r.is_err(),
        "introducer must refuse a peer key not in the offer: {r:?}"
    );
    conn.close();
    let _ = tokio::time::timeout(Duration::from_secs(5), newcomer_task).await;
}

/// The connection drops after the hello: the newcomer aborts and ends up
/// neither joined nor confirmed.
#[tokio::test]
async fn a_connection_lost_mid_ceremony_aborts_the_newcomer() {
    let laptop_binding = SigningKey::generate().unwrap();
    let mut laptop = side(&laptop_binding);
    let newcomer = Newcomer::new(
        laptop_binding.clone(),
        SigningKey::generate().unwrap(),
        "laptop",
        "linux",
        now(),
    )
    .unwrap();
    let offer = newcomer.offer(vec![laptop.manager.endpoint().local_addr().unwrap()]);
    let code = offer.code.clone();

    let newcomer_task = tokio::spawn(async move {
        let (conn, stream) = tokio::time::timeout(Duration::from_secs(5), laptop.streams.recv())
            .await
            .unwrap()
            .unwrap();
        // The user never gets to answer: the connection is gone first.
        let (code_tx, _code_rx) = mpsc::channel(1);
        let (_confirm_tx, confirm_rx) = oneshot::channel::<bool>();
        let p = Prompt {
            code: code_tx,
            confirm: confirm_rx,
        };
        run_newcomer(&conn, stream, newcomer, p).await
    });

    // A stranger dials with the right one-time code, says hello, then vanishes.
    let stranger_key = SigningKey::generate().unwrap();
    let stranger = side(&stranger_key);
    let conn = stranger.manager.connect(offer.addrs[0]).await.unwrap();
    let mut stream = open_on(&conn, PURPOSE).await.unwrap();
    stream
        .send(&lp_identity::pairing::Hello {
            message_type: lp_core::wire::TypeName::new(types::HELLO),
            code,
        })
        .await
        .unwrap();
    // Wait for the ready, then drop everything.
    let _ready: Option<lp_identity::pairing::Ready> = stream.recv().await.unwrap();
    drop(stream);
    stranger.manager.shutdown();

    let r = tokio::time::timeout(Duration::from_secs(10), newcomer_task)
        .await
        .expect("newcomer notices the loss")
        .unwrap();
    assert!(r.is_err(), "newcomer must abort: {r:?}");
}

/// A grant captured from one ceremony is replayed to a fresh newcomer with
/// different keys: the roles name the wrong keys and it is refused.
#[test]
fn a_replayed_grant_is_refused_by_a_fresh_newcomer() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, now()).unwrap();
    let intro_key = *SigningKey::generate().unwrap().public();

    let first = Newcomer::new(
        SigningKey::generate().unwrap(),
        SigningKey::generate().unwrap(),
        "a",
        "linux",
        now(),
    )
    .unwrap();
    let first_offer = first.offer(vec!["127.0.0.1:1".parse().unwrap()]);
    let captured = Grant {
        message_type: lp_core::wire::TypeName::new(types::GRANT),
        person: person.clone(),
        roles: grant_roles(
            &alice.self_signing,
            *alice.person(),
            &first_offer,
            true,
            now(),
        )
        .unwrap(),
        self_signing: None,
        gossip: vec![],
    };
    let _ = first;

    let mut second = Newcomer::new(
        SigningKey::generate().unwrap(),
        SigningKey::generate().unwrap(),
        "b",
        "linux",
        now(),
    )
    .unwrap();
    let second_offer = second.offer(vec!["127.0.0.1:1".parse().unwrap()]);
    second
        .on_hello(
            intro_key,
            &lp_identity::pairing::Hello {
                message_type: lp_core::wire::TypeName::new(types::HELLO),
                code: second_offer.code.clone(),
            },
        )
        .unwrap();
    second.confirm().unwrap();
    assert!(second.on_grant(captured).is_err());
    assert_eq!(second.state(), NewcomerState::Aborted);
    let _ = Offer::parse(&second_offer.to_text()).unwrap();
}
