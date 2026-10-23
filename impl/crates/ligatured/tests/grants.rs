//! The first clipboard use by a binding of the same person records one
//! implicit grant; a stranger gets none; grants are never gossiped.

use ligatured::control::{Client, Request, serve, types};
use ligatured::daemon::Daemon;
use ligatured::grants::{Grants, exists, grant_implicit, list};
use lp_core::Error;
use lp_core::capability::CLIPBOARD_TYPE;
use lp_core::facts::{GrantBasis, Role};
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_crypto::signing::SigningKey;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::gossip;

#[tokio::test]
async fn implicit_grants_are_recorded_once_listed_and_kept_local() {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (d, _events) =
        Daemon::open(loc.clone(), "127.0.0.1:0".parse().unwrap(), Some("laptop")).unwrap();
    let alice = PersonKeys::generate().unwrap();
    let at = Timestamp::from_unix(1_760_000_000).unwrap();
    d.store()
        .insert(&facts::person_fact(&alice, "Alice", 1, at).unwrap())
        .unwrap();
    d.store()
        .insert(&facts::role_fact(&alice, Role::Binding, *d.binding().public(), true, at).unwrap())
        .unwrap();
    d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
        .unwrap();
    let phone = SigningKey::generate().unwrap();
    d.store()
        .insert(&facts::role_fact(&alice, Role::Binding, *phone.public(), true, at).unwrap())
        .unwrap();

    let before = d.store().ids().unwrap().len();
    assert!(
        grant_implicit(&d, phone.public(), CLIPBOARD_TYPE).unwrap(),
        "first use records"
    );
    assert!(
        !grant_implicit(&d, phone.public(), CLIPBOARD_TYPE).unwrap(),
        "second use does not"
    );
    assert!(exists(&d, phone.public(), CLIPBOARD_TYPE).unwrap());
    let grants = list(&d).unwrap().grants;
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].grantee, *phone.public());
    assert_eq!(grants[0].grantor, *d.binding().public());
    assert_eq!(grants[0].basis, GrantBasis::Implicit);
    assert_eq!(grants[0].capability, CLIPBOARD_TYPE);

    // Never gossiped: the identifiers offered to peers do not grow.
    assert_eq!(d.store().ids().unwrap().len(), before);
    let have = gossip::have(d.store()).unwrap();
    assert_eq!(have.facts.len(), before);
    let everything = lp_core::frame::GossipWant::new(
        d.store().all().unwrap().into_iter().map(|f| f.id).collect(),
    );
    assert!(
        gossip::facts_for(d.store(), &everything)
            .unwrap()
            .iter()
            .all(|f| f.envelope.fact_type.as_str() != lp_core::facts::GRANT_TYPE)
    );
    // Nor handed to a newcomer at pairing (LP-128).
    assert_eq!(d.all_facts().unwrap().len(), before);
    assert!(
        d.all_facts()
            .unwrap()
            .iter()
            .all(|e| e.fact_type.as_str() != lp_core::facts::GRANT_TYPE)
    );

    // A stranger is refused.
    let stranger = SigningKey::generate().unwrap();
    assert!(matches!(
        grant_implicit(&d, stranger.public(), CLIPBOARD_TYPE),
        Err(Error::Chain(_))
    ));
    assert_eq!(list(&d).unwrap().grants.len(), 1);

    // lpctl can list it.
    let _server = serve(d.clone(), &loc.control_socket()).unwrap();
    let mut ctl = Client::connect(&loc.control_socket()).await.unwrap();
    let g: Grants = ctl.request(Request::new(types::GRANTS, 0)).await.unwrap();
    assert_eq!(g.grants.len(), 1);
    d.shutdown();
}
