//! Two daemons of the same person converge after one gossip exchange, with
//! a revocation among the facts, and a bad fact is discarded.

use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::gossip::{exchange, run_dialer};
use lp_core::facts::{RevocationKind, Role};
use lp_core::frame::{GossipFact, StreamHeader};
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_crypto::signing::SigningKey;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::trust::Recognition;
use lp_identity::trust_store::TrustStore;
use lp_transport::manager::open_on;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

#[tokio::test]
async fn two_stores_converge_and_the_revocation_lands_first() {
    // Alice's facts, split across two daemons that both already trust each other.
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let (a, _ea) = Daemon::open(
        Locations::resolve(Some(d1.path()), None),
        "127.0.0.1:0".parse().unwrap(),
        Some("a"),
    )
    .unwrap();
    let (b, _eb) = Daemon::open(
        Locations::resolve(Some(d2.path()), None),
        "127.0.0.1:0".parse().unwrap(),
        Some("b"),
    )
    .unwrap();
    let a_role =
        facts::role_fact(&alice, Role::Binding, *a.binding().public(), true, at(1)).unwrap();
    let b_role =
        facts::role_fact(&alice, Role::Binding, *b.binding().public(), true, at(2)).unwrap();
    for d in [&a, &b] {
        d.store().insert(&person).unwrap();
        d.store().insert(&a_role).unwrap();
        d.store().insert(&b_role).unwrap();
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    // Only A knows about the old phone and its revocation; only B knows a TV.
    let phone = SigningKey::generate().unwrap();
    let phone_role =
        facts::role_fact(&alice, Role::Binding, *phone.public(), false, at(3)).unwrap();
    let phone_rev =
        facts::revocation_fact(&alice, *phone.public(), RevocationKind::Revocation, at(4)).unwrap();
    a.store().insert(&phone_role).unwrap();
    a.store().insert(&phone_rev).unwrap();
    let tv = SigningKey::generate().unwrap();
    let tv_role = facts::role_fact(&alice, Role::Principal, *tv.public(), false, at(5)).unwrap();
    b.store().insert(&tv_role).unwrap();

    // A dials B; the manager hands B's side to the registered handler.
    let conn = a
        .manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    assert_eq!(
        a.recognise(conn.peer()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    let result = tokio::time::timeout(Duration::from_secs(5), run_dialer(a.store(), &conn))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.sent, 2, "phone role and its revocation");
    assert_eq!(result.stored, 1, "the tv");
    assert_eq!(result.discarded, 0);

    // Give B's handler a moment to finish storing.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(a.store().ids().unwrap().len(), 6);
    assert_eq!(b.store().ids().unwrap().len(), 6);
    assert!(
        lp_identity::trust::is_revoked(b.store(), alice.person(), phone.public()).unwrap(),
        "revocation reached B"
    );
    assert_eq!(b.recognise(phone.public()).unwrap(), Recognition::Revoked);
    assert_eq!(a.recognise(tv.public()).unwrap(), Recognition::MyPrincipal);

    // A second exchange has nothing to do.
    let again = run_dialer(a.store(), &conn).await.unwrap();
    assert_eq!(again, ligatured::gossip::Exchanged::default());
    a.shutdown();
    b.shutdown();
}

#[tokio::test]
async fn a_tampered_fact_from_a_peer_is_discarded() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let store = Arc::new(TrustStore::in_memory().unwrap());

    // A raw peer that follows the exchange but sends one bad fact.
    let key_a = SigningKey::generate().unwrap();
    let key_b = SigningKey::generate().unwrap();
    let ep_a = lp_transport::quic::Endpoint::bind("127.0.0.1:0".parse().unwrap(), &key_a).unwrap();
    let ep_b = lp_transport::quic::Endpoint::bind("127.0.0.1:0".parse().unwrap(), &key_b).unwrap();
    let (ma, _) = lp_transport::manager::Manager::start(ep_a);
    let (mb, _) = lp_transport::manager::Manager::start(ep_b);
    ligatured::gossip::register_store(&mb, store.clone());
    let conn = ma
        .connect(mb.endpoint().local_addr().unwrap())
        .await
        .unwrap();
    let mut stream = open_on(&conn, StreamHeader::GOSSIP).await.unwrap();

    let mut bad = person.clone();
    bad.payload.0[9] ^= 0x80;
    let good = facts::role_fact(&alice, Role::Binding, *key_b.public(), true, at(1)).unwrap();
    // Our have lists both; the peer wants both; we send the bad then the good.
    stream
        .send(&lp_core::frame::GossipHave::new(vec![
            facts::id(&bad),
            facts::id(&good),
        ]))
        .await
        .unwrap();
    let _their_have: lp_core::frame::GossipHave = stream.recv().await.unwrap().unwrap();
    stream
        .send(&lp_core::frame::GossipWant::new(vec![]))
        .await
        .unwrap();
    let want: lp_core::frame::GossipWant = stream.recv().await.unwrap().unwrap();
    assert_eq!(want.facts.len(), 2);
    stream.send(&GossipFact::new(bad)).await.unwrap();
    stream.send(&GossipFact::new(good.clone())).await.unwrap();
    stream.finish();
    while stream.recv::<GossipFact>().await.unwrap().is_some() {}
    tokio::time::sleep(Duration::from_millis(200)).await;
    let ids = store.ids().unwrap();
    assert_eq!(ids, vec![facts::id(&good)], "only the good fact was stored");
    let _ = exchange; // the same function runs on both sides
    ma.shutdown();
    mb.shutdown();
}
