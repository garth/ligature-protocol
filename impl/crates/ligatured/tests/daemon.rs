//! The daemon starts without an identity, gets one from `init`, and keeps it
//! across a restart.

use ligatured::daemon::Daemon;
use lp_core::Error;
use lp_core::paths::Locations;
use lp_identity::trust::Recognition;

#[tokio::test]
async fn init_then_restart_keeps_the_identity() {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let bind = "127.0.0.1:0".parse().unwrap();

    let (daemon, _events) = Daemon::open(loc.clone(), bind, Some("laptop")).unwrap();
    let before = daemon.status();
    assert!(!before.initialised);
    assert!(before.person.is_none());
    assert_eq!(before.device_name, "laptop");
    assert_ne!(before.port, 0);
    assert!(matches!(
        daemon.recognise(&before.binding),
        Err(Error::NotInitialised)
    ));
    assert!(
        daemon.with_mdns(|_| ()).is_none(),
        "nothing advertised before init"
    );

    let person = daemon.init("Alice", None).unwrap();
    assert!(matches!(
        daemon.init("Alice", None),
        Err(Error::AlreadyInitialised)
    ));
    let after = daemon.status();
    assert!(after.initialised);
    assert_eq!(after.person, Some(person));
    assert_eq!(after.name.as_deref(), Some("Alice"));
    assert_eq!(after.binding, before.binding);
    assert!(daemon.with_mdns(|_| ()).is_some(), "advertising after init");
    assert!(
        daemon.self_signing().unwrap().is_some(),
        "the first device is an introducer"
    );
    // Our own binding is one of our bindings.
    assert_eq!(
        daemon.recognise(&after.binding).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    assert_eq!(
        daemon.recognise(&after.principal).unwrap(),
        Recognition::MyPrincipal
    );
    daemon.shutdown();
    drop(daemon);

    let (again, _events) = Daemon::open(loc, bind, None).unwrap();
    let restarted = again.status();
    assert!(restarted.initialised);
    assert_eq!(restarted.person, Some(person));
    assert_eq!(restarted.binding, before.binding);
    assert_eq!(restarted.principal, before.principal);
    assert_eq!(
        restarted.device_name, "laptop",
        "name comes from the stored record"
    );
    assert_eq!(restarted.name.as_deref(), Some("Alice"));
    again.shutdown();
}
