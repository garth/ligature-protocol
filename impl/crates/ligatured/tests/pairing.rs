//! The ceremony over real QUIC on loopback: both sides derive the same code,
//! both users confirm, the newcomer joins with the self-signing key; and a
//! user who says the codes differ aborts it.

use std::sync::Arc;
use std::time::Duration;

use ligatured::ceremony::{Prompt, run_introducer, run_newcomer};
use lp_core::wire::Timestamp;
use lp_crypto::signing::SigningKey;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::pairing::{Introducer, Newcomer, Offer, PURPOSE};
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

async fn ceremony(
    newcomer_says: bool,
    introducer_says: bool,
) -> (
    Result<(), String>,
    Result<(), String>,
    Option<(String, String)>,
) {
    // Alice already has a phone with her keys; the laptop is new.
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, now()).unwrap();
    let phone_key = SigningKey::generate().unwrap();
    let phone = side(&phone_key);

    let laptop_binding = SigningKey::generate().unwrap();
    let laptop_principal = SigningKey::generate().unwrap();
    let mut laptop = side(&laptop_binding);
    let newcomer = Newcomer::new(
        laptop_binding.clone(),
        laptop_principal,
        "laptop",
        "linux",
        now(),
    )
    .unwrap();
    let offer_text = newcomer
        .offer(vec![laptop.manager.endpoint().local_addr().unwrap()])
        .to_text();

    let (n_code_tx, mut n_code_rx) = mpsc::channel(1);
    let (n_confirm_tx, n_confirm_rx) = oneshot::channel();
    let expected_person = person.clone();
    let newcomer_task = tokio::spawn(async move {
        let (conn, stream) = tokio::time::timeout(Duration::from_secs(5), laptop.streams.recv())
            .await
            .expect("introducer dialled")
            .expect("handler alive");
        run_newcomer(
            &conn,
            stream,
            newcomer,
            Prompt {
                code: n_code_tx,
                confirm: n_confirm_rx,
            },
        )
        .await
        .map(|joined| {
            assert_eq!(joined.person, expected_person);
            assert!(joined.self_signing.is_some());
            assert_eq!(joined.roles.len(), 2);
        })
        .map_err(|e| e.to_string())
    });

    // The introducer scans the offer and dials.
    let offer = Offer::parse(&offer_text).unwrap();
    let conn = phone.manager.connect(offer.addrs[0]).await.unwrap();
    let stream = open_on(&conn, PURPOSE).await.unwrap();
    let introducer = Introducer::new(
        alice.clone(),
        person.clone(),
        *phone_key.public(),
        offer,
        true,
        vec![],
    );
    let (i_code_tx, mut i_code_rx) = mpsc::channel(1);
    let (i_confirm_tx, i_confirm_rx) = oneshot::channel();
    let introducer_task = tokio::spawn(async move {
        run_introducer(
            &conn,
            stream,
            introducer,
            Prompt {
                code: i_code_tx,
                confirm: i_confirm_rx,
            },
            now(),
        )
        .await
        .map(|introduced| {
            assert_eq!(introduced.devices.len(), 2);
            assert_eq!(introduced.roles.len(), 2);
        })
        .map_err(|e| e.to_string())
    });

    let codes = match (
        tokio::time::timeout(Duration::from_secs(5), n_code_rx.recv()).await,
        tokio::time::timeout(Duration::from_secs(5), i_code_rx.recv()).await,
    ) {
        (Ok(Some(n)), Ok(Some(i))) => Some((n, i)),
        _ => None,
    };
    let _ = n_confirm_tx.send(newcomer_says);
    let _ = i_confirm_tx.send(introducer_says);
    let n = tokio::time::timeout(Duration::from_secs(5), newcomer_task)
        .await
        .unwrap()
        .unwrap();
    let i = tokio::time::timeout(Duration::from_secs(5), introducer_task)
        .await
        .unwrap()
        .unwrap();
    phone.manager.shutdown();
    (n, i, codes)
}

#[tokio::test]
async fn both_sides_show_the_same_code_and_the_newcomer_joins() {
    let (n, i, codes) = ceremony(true, true).await;
    let (n_code, i_code) = codes.expect("both sides showed a code");
    assert_eq!(n_code, i_code);
    assert_eq!(n_code.len(), 6);
    assert!(n_code.bytes().all(|b| b.is_ascii_digit()));
    n.unwrap();
    i.unwrap();
}

#[tokio::test]
async fn a_user_who_sees_different_codes_aborts_the_ceremony() {
    let (n, i, codes) = ceremony(true, false).await;
    assert!(codes.is_some());
    assert!(n.is_err(), "newcomer learns of the abort: {n:?}");
    assert!(i.is_err(), "introducer aborted: {i:?}");
}
