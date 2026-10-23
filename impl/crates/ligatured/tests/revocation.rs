//! Three daemons of one person: A revokes C; B learns it from A; C can no
//! longer be recognised by B, whether B hears by push or on the next session.

use std::sync::Arc;
use std::time::Duration;

use ligatured::control::{Client, Empty, Request, serve, types};
use ligatured::daemon::Daemon;
use ligatured::gossip::run_dialer;
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::trust::Recognition;

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

/// A daemon driven by hand: its streams are admitted without the event loop
/// (LP-121), so a test dials and gossips directly.
fn daemon(name: &str) -> (Arc<Daemon>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (d, _events) = Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some(name)).unwrap();
    d.manager().admit_all();
    (d, dir)
}

/// A daemon with the real event loop, which admits trusted peers and refuses
/// the rest, as `run` does.
fn daemon_with_loop(name: &str) -> (Arc<Daemon>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (d, events) = Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some(name)).unwrap();
    ligatured::run::spawn_connection_events(Arc::clone(&d), events);
    (d, dir)
}

#[tokio::test]
async fn a_revocation_made_on_one_device_reaches_the_others() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let (a, da) = daemon("a");
    let (b, _db) = daemon_with_loop("b");
    let (c, _dc) = daemon("c");
    let roles: Vec<_> = [&a, &b, &c]
        .iter()
        .enumerate()
        .map(|(i, d)| {
            facts::role_fact(
                &alice,
                Role::Binding,
                *d.binding().public(),
                true,
                at(1 + i as i64),
            )
            .unwrap()
        })
        .collect();
    for d in [&a, &b, &c] {
        d.store().insert(&person).unwrap();
        for r in &roles {
            d.store().insert(r).unwrap();
        }
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    assert_eq!(
        b.recognise(c.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );

    // A is connected to B; A revokes C through its control channel.
    let loc_a = Locations::resolve(Some(da.path()), None);
    let _server = serve(a.clone(), &loc_a.control_socket()).unwrap();
    let mut ctl = Client::connect(&loc_a.control_socket()).await.unwrap();
    let ab = a
        .manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    assert_eq!(
        a.recognise(ab.peer()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    let mut revoke = Request::new(types::REVOKE, 0);
    revoke.key = Some(*c.binding().public());
    let _: Empty = ctl.request(revoke).await.unwrap();
    assert_eq!(
        a.recognise(c.binding().public()).unwrap(),
        Recognition::Revoked
    );

    // B heard by push over the live connection.
    until("pushed to B", || {
        b.recognise(c.binding().public()).unwrap() == Recognition::Revoked
    })
    .await;

    // C connects to B: authenticated, but B's event loop sees a revoked key
    // on its Connected event and drops it without a hand from the test.
    let cb = c
        .manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    assert_eq!(cb.peer(), b.binding().public());
    until("B refused C", || {
        b.manager().peer(c.binding().public()).is_none()
    })
    .await;

    // A fourth device that was offline learns on its next session with anyone.
    // It is one of Alice's bindings, known to the others, since a stranger is
    // dropped after an exchange (LP-122).
    let (d, _dd) = daemon("d");
    d.store().insert(&person).unwrap();
    for r in &roles {
        d.store().insert(r).unwrap();
    }
    let d_role =
        facts::role_fact(&alice, Role::Binding, *d.binding().public(), true, at(10)).unwrap();
    for x in [&a, &b, &c, &d] {
        x.store().insert(&d_role).unwrap();
    }
    d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
        .unwrap();
    assert_eq!(
        d.recognise(c.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    let db_conn = d
        .manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    let result = run_dialer(d.store(), &db_conn).await.unwrap();
    assert_eq!(result.stored, 1, "the revocation");
    assert_eq!(
        d.recognise(c.binding().public()).unwrap(),
        Recognition::Revoked
    );

    // Revocation is not undone by gossip of the old role facts.
    assert!(a.store().about(c.binding().public()).unwrap().len() >= 2);
    for x in [&a, &b, &c, &d] {
        x.shutdown();
    }
}

/// A revocation that a stranger signed, naming a binding or the master key,
/// is stored when it arrives by gossip and changes nothing: recognition goes
/// by the chain, not by the subject (LP-120).
#[tokio::test]
async fn a_forged_revocation_relayed_by_gossip_changes_nothing() {
    use lp_core::facts::{RevocationFact, RevocationKind};
    use lp_crypto::signing::SigningKey;

    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let (a, _da) = daemon("a");
    let (b, _db) = daemon("b");
    for (i, d) in [&a, &b].iter().enumerate() {
        d.store().insert(&person).unwrap();
        for x in [&a, &b] {
            d.store()
                .insert(
                    &facts::role_fact(
                        &alice,
                        Role::Binding,
                        *x.binding().public(),
                        true,
                        at(1 + i as i64),
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }

    // Mallory's forgeries reach A somehow (a compromised peer relayed them);
    // A stores them, since they are well-formed self-signed facts.
    let mallory = SigningKey::generate().unwrap();
    for subject in [*b.binding().public(), *alice.person()] {
        let forged = RevocationFact {
            subject,
            kind: RevocationKind::Revocation,
            person: *alice.person(),
            revoked_at: at(5),
        };
        a.store()
            .insert(&facts::sign(&mallory, &forged).unwrap())
            .unwrap();
    }
    assert_eq!(
        a.recognise(b.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: true },
        "A itself is unmoved"
    );

    // Gossip carries them to B, revocations first; B is unmoved too.
    let ab = a
        .manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    let result = run_dialer(a.store(), &ab).await.unwrap();
    assert!(result.sent >= 2, "the two forgeries, among the rest");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        b.recognise(a.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    assert!(
        !lp_identity::trust::is_revoked(b.store(), alice.person(), alice.person()).unwrap(),
        "the master key is not abandoned on a stranger's say-so"
    );
    let listed = ligatured::control::devices(&b).unwrap();
    assert!(listed.devices.iter().all(|d| !d.revoked));

    // Alice's own revocation of A, from B, still works after the forgeries.
    b.store()
        .insert(
            &facts::revocation_fact(
                &alice,
                *a.binding().public(),
                RevocationKind::Revocation,
                at(6),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        b.recognise(a.binding().public()).unwrap(),
        Recognition::Revoked
    );
    for x in [&a, &b] {
        x.shutdown();
    }
}

/// A device that learns a revocation by gossip closes the session it holds
/// with the revoked device at once, so the revoked device gains nothing from
/// a session it held before (chapter 06 section 5, LP-122).
#[tokio::test]
async fn a_revocation_learnt_by_gossip_closes_the_live_session() {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let (a, _da) = daemon("a");
    let (b, _db) = daemon("b");
    let (c, _dc) = daemon("c");
    let roles: Vec<_> = [&a, &b, &c]
        .iter()
        .enumerate()
        .map(|(i, d)| {
            facts::role_fact(
                &alice,
                Role::Binding,
                *d.binding().public(),
                true,
                at(1 + i as i64),
            )
            .unwrap()
        })
        .collect();
    for d in [&a, &b, &c] {
        d.store().insert(&person).unwrap();
        for r in &roles {
            d.store().insert(r).unwrap();
        }
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }

    // C holds a live, trusted session with B.
    let cb = c
        .manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    assert_eq!(cb.peer(), b.binding().public());
    until("B holds C's session", || {
        b.manager().peer(c.binding().public()).is_some()
    })
    .await;
    assert_eq!(
        b.recognise(c.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );

    // A, connected to B, revokes C and pushes the revocation.
    let _ab = a
        .manager()
        .connect(b.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    ligatured::control::revoke_and_push(&a, c.binding().public())
        .await
        .unwrap();

    // B learnt it and closed C's session; C sees its connection go.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while b.manager().peer(c.binding().public()).is_some()
        || c.manager().peer(b.binding().public()).is_some()
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "B still holds C's session after learning the revocation"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        b.recognise(c.binding().public()).unwrap(),
        Recognition::Revoked
    );
    for x in [&a, &b, &c] {
        x.shutdown();
    }
}
