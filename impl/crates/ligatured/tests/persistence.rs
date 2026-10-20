//! Restarting daemons keeps keys and trust: after pairing, both restart from
//! disk and recognise each other with no new ceremony; a revocation made
//! while one was down is learned on reconnect.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ligatured::control::{Client, Event, InitResult, Request, revoke_and_push, serve, types};
use ligatured::daemon::Daemon;
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_crypto::signing::SigningKey;
use lp_identity::trust::Recognition;
use lp_transport::manager::Event as PeerEvent;

fn open(
    dir: &std::path::Path,
    name: &str,
) -> (Arc<Daemon>, tokio::sync::mpsc::Receiver<PeerEvent>) {
    // Every interface, so the pairing offer's addresses are dialable.
    Daemon::open(
        Locations::resolve(Some(dir), None),
        "0.0.0.0:0".parse().unwrap(),
        Some(name),
    )
    .unwrap()
}

fn drive(d: Arc<Daemon>, mut events: tokio::sync::mpsc::Receiver<PeerEvent>) {
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            match ev {
                PeerEvent::Connected(conn) => {
                    if d.recognise(conn.peer())
                        .map(|r| r.is_trusted())
                        .unwrap_or(false)
                    {
                        let d = d.clone();
                        tokio::spawn(async move { d.on_trusted(&conn).await });
                    }
                }
                PeerEvent::Disconnected(peer) => d.on_disconnected(&peer),
            }
        }
    });
}

fn loopback_of(d: &Daemon) -> std::net::SocketAddr {
    let port = d.manager().endpoint().local_addr().unwrap().port();
    std::net::SocketAddr::from(([127, 0, 0, 1], port))
}

fn confirm(event: &Event) -> Request {
    let mut r = Request::new(types::PAIR_CONFIRM, 0);
    r.pairing = Some(event.id);
    r
}

async fn pair(
    phone: &Arc<Daemon>,
    phone_loc: &Locations,
    laptop: &Arc<Daemon>,
    laptop_loc: &Locations,
) {
    let _s1 = serve(phone.clone(), &phone_loc.control_socket()).unwrap();
    let _s2 = serve(laptop.clone(), &laptop_loc.control_socket()).unwrap();
    let mut phone_ctl = Client::connect(&phone_loc.control_socket()).await.unwrap();
    let mut laptop_ctl = Client::connect(&laptop_loc.control_socket()).await.unwrap();
    let mut init = Request::new(types::INIT, 0);
    init.name = Some("Alice".to_owned());
    let _: InitResult = phone_ctl.request(init).await.unwrap();

    let (offer_tx, offer_rx) = tokio::sync::oneshot::channel::<String>();
    let offer_tx = Arc::new(Mutex::new(Some(offer_tx)));
    let join = tokio::spawn(async move {
        laptop_ctl
            .call(
                Request::new(types::PAIR_JOIN, 0),
                |event: &Event| match event.event_type.as_str() {
                    types::PAIR_OFFER => {
                        if let Some(tx) = offer_tx.lock().unwrap().take() {
                            let _ = tx.send(event.offer.clone().unwrap());
                        }
                        None
                    }
                    types::PAIR_CODE => Some(confirm(event)),
                    _ => None,
                },
            )
            .await
    });
    let offer = tokio::time::timeout(Duration::from_secs(5), offer_rx)
        .await
        .unwrap()
        .unwrap();
    let mut introduce = Request::new(types::PAIR_INTRODUCE, 0);
    introduce.offer = Some(offer);
    let r = tokio::time::timeout(
        Duration::from_secs(10),
        phone_ctl.call(introduce, |event: &Event| {
            (event.event_type.as_str() == types::PAIR_CODE).then(|| confirm(event))
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(r.error.is_none(), "{:?}", r.error);
    let j = tokio::time::timeout(Duration::from_secs(10), join)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(j.error.is_none(), "{:?}", j.error);
}

#[tokio::test]
async fn restart_keeps_the_pairing_and_catches_up_on_revocations() {
    let phone_dir = tempfile::tempdir().unwrap();
    let laptop_dir = tempfile::tempdir().unwrap();
    let phone_loc = Locations::resolve(Some(phone_dir.path()), None);
    let laptop_loc = Locations::resolve(Some(laptop_dir.path()), None);

    // Pair, then shut both down.
    let (phone, pe) = open(phone_dir.path(), "phone");
    let (laptop, le) = open(laptop_dir.path(), "laptop");
    let phone_key = *phone.binding().public();
    let laptop_key = *laptop.binding().public();
    drive(phone.clone(), pe);
    drive(laptop.clone(), le);
    pair(&phone, &phone_loc, &laptop, &laptop_loc).await;
    let person = phone.person().unwrap();
    phone.shutdown();
    laptop.shutdown();
    drop(phone);
    drop(laptop);

    // Restart both: same keys, same person, recognised without a ceremony.
    let (phone, pe) = open(phone_dir.path(), "phone");
    let (laptop, le) = open(laptop_dir.path(), "laptop");
    assert_eq!(*phone.binding().public(), phone_key);
    assert_eq!(*laptop.binding().public(), laptop_key);
    assert_eq!(phone.person(), Some(person));
    assert_eq!(laptop.person(), Some(person));
    assert_eq!(
        laptop.recognise(&phone_key).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    assert_eq!(
        phone.recognise(&laptop_key).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    drive(phone.clone(), pe);
    drive(laptop.clone(), le);
    let conn = laptop.manager().connect(loopback_of(&phone)).await.unwrap();
    assert_eq!(conn.peer(), &phone_key);
    assert!(laptop.recognise(conn.peer()).unwrap().is_trusted());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(phone.manager().peers(), vec![laptop_key]);

    // The laptop goes down; the phone revokes a third device meanwhile.
    laptop.shutdown();
    drop(laptop);
    let tablet = SigningKey::generate().unwrap();
    let self_signing = phone.self_signing().unwrap().unwrap();
    let role = lp_identity::facts::role_fact_by(
        &self_signing,
        person,
        Role::Binding,
        *tablet.public(),
        false,
        Timestamp::now(),
    )
    .unwrap();
    phone.store().insert(&role).unwrap();
    revoke_and_push(&phone, tablet.public()).await.unwrap();
    assert_eq!(
        phone.recognise(tablet.public()).unwrap(),
        Recognition::Revoked
    );

    // The laptop comes back and learns it on reconnect.
    let (laptop, le) = open(laptop_dir.path(), "laptop");
    drive(laptop.clone(), le);
    assert!(
        matches!(laptop.recognise(tablet.public()), Ok(Recognition::Stranger)),
        "not yet known at all"
    );
    laptop.manager().connect(loopback_of(&phone)).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if laptop.recognise(tablet.public()).unwrap() == Recognition::Revoked {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "revocation not learned on reconnect"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    phone.shutdown();
    laptop.shutdown();
}
