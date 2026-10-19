//! The control socket: status, init, devices, errors, over a real Unix socket.

use std::sync::Arc;

use ligatured::control::{Client, Devices, InitResult, Request, serve, types};
use ligatured::daemon::{Daemon, Status};
use lp_core::paths::Locations;

#[tokio::test]
async fn status_init_devices_and_errors_over_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (daemon, _events) =
        Daemon::open(loc.clone(), "127.0.0.1:0".parse().unwrap(), Some("laptop")).unwrap();
    let daemon: Arc<Daemon> = daemon;
    let _server = serve(daemon.clone(), &loc.control_socket()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(loc.control_socket())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    let mut client = Client::connect(&loc.control_socket()).await.unwrap();
    let status: Status = client
        .request(Request::new(types::STATUS, 0))
        .await
        .unwrap();
    assert!(!status.initialised);
    assert_eq!(status.device_name, "laptop");

    let err = client
        .call(Request::new(types::DEVICES, 0), |_| {})
        .await
        .unwrap();
    assert_eq!(
        err.error.as_ref().map(|e| e.code.as_str()),
        Some("not-initialised")
    );

    let mut init = Request::new(types::INIT, 0);
    init.name = Some("Alice".to_owned());
    let created: InitResult = client.request(init.clone()).await.unwrap();
    let again = client.call(init, |_| {}).await.unwrap();
    assert_eq!(
        again.error.as_ref().map(|e| e.code.as_str()),
        Some("already-initialised")
    );

    let status: Status = client
        .request(Request::new(types::STATUS, 0))
        .await
        .unwrap();
    assert!(status.initialised);
    assert_eq!(status.person, Some(created.person));
    assert_eq!(status.name.as_deref(), Some("Alice"));

    let devices: Devices = client
        .request(Request::new(types::DEVICES, 0))
        .await
        .unwrap();
    assert_eq!(devices.devices.len(), 2, "binding and principal");
    assert!(devices.devices.iter().all(|d| d.this_device));
    assert!(
        devices
            .devices
            .iter()
            .any(|d| d.role == "binding" && d.introducer)
    );
    assert_eq!(devices.devices[0].display_name.as_deref(), Some("laptop"));

    let unknown = client
        .call(Request::new("org.example.nothing", 0), |_| {})
        .await
        .unwrap();
    assert_eq!(
        unknown.error.as_ref().map(|e| e.code.as_str()),
        Some("unknown-type")
    );

    // A second client on the same socket works independently.
    let mut other = Client::connect(&loc.control_socket()).await.unwrap();
    let s: Status = other.request(Request::new(types::STATUS, 0)).await.unwrap();
    assert!(s.initialised);
    daemon.shutdown();
}

#[tokio::test]
async fn a_stale_socket_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    std::fs::create_dir_all(&loc.data_dir).unwrap();
    std::fs::write(loc.control_socket(), b"stale").unwrap();
    let (daemon, _events) =
        Daemon::open(loc.clone(), "127.0.0.1:0".parse().unwrap(), None).unwrap();
    let _server = serve(daemon.clone(), &loc.control_socket()).unwrap();
    let mut client = Client::connect(&loc.control_socket()).await.unwrap();
    let _: Status = client
        .request(Request::new(types::STATUS, 0))
        .await
        .unwrap();
    daemon.shutdown();
}
