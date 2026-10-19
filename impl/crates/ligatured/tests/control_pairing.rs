//! Two daemons pair through their control channels: the newcomer's client
//! gets the offer and the code and confirms; the introducer's client is given
//! the offer and confirms; afterwards both list each other in `devices`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ligatured::control::{Client, Devices, Event, InitResult, Request, serve, types};
use ligatured::daemon::{Daemon, Status};
use lp_core::paths::Locations;
use lp_identity::trust::Recognition;

async fn start(name: &str) -> (Arc<Daemon>, Client, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(dir.path()), None);
    let (daemon, _events) =
        Daemon::open(loc.clone(), "0.0.0.0:0".parse().unwrap(), Some(name)).unwrap();
    let _server = serve(daemon.clone(), &loc.control_socket()).unwrap();
    let client = Client::connect(&loc.control_socket()).await.unwrap();
    (daemon, client, dir)
}

fn confirm(event: &Event) -> Request {
    let mut r = Request::new(types::PAIR_CONFIRM, 0);
    r.pairing = Some(event.id);
    r
}

#[tokio::test]
async fn two_daemons_pair_through_their_control_channels() {
    let (phone, mut phone_ctl, _d1) = start("phone").await;
    let (laptop, mut laptop_ctl, _d2) = start("laptop").await;

    let mut init = Request::new(types::INIT, 0);
    init.name = Some("Alice".to_owned());
    let _: InitResult = phone_ctl.request(init).await.unwrap();

    // The laptop joins: it shows an offer, then a code, and confirms.
    let laptop_code: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let (offer_tx, offer_rx) = tokio::sync::oneshot::channel::<String>();
    let offer_tx = Arc::new(Mutex::new(Some(offer_tx)));
    let join = {
        let laptop_code = laptop_code.clone();
        tokio::spawn(async move {
            let response = laptop_ctl
                .call(
                    Request::new(types::PAIR_JOIN, 0),
                    |event: &Event| match event.event_type.as_str() {
                        types::PAIR_OFFER => {
                            if let Some(tx) = offer_tx.lock().unwrap().take() {
                                let _ = tx.send(event.offer.clone().unwrap());
                            }
                            None
                        }
                        types::PAIR_CODE => {
                            *laptop_code.lock().unwrap() = event.code.clone();
                            Some(confirm(event))
                        }
                        _ => None,
                    },
                )
                .await;
            (response, laptop_ctl)
        })
    };

    // The phone introduces with the offer it "scanned".
    let offer_text = tokio::time::timeout(Duration::from_secs(5), offer_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(offer_text.starts_with("ligature:pair/1?"));
    let phone_code: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut introduce = Request::new(types::PAIR_INTRODUCE, 0);
    introduce.offer = Some(offer_text);
    let pc = phone_code.clone();
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        phone_ctl.call(introduce, move |event: &Event| {
            if event.event_type.as_str() == types::PAIR_CODE {
                *pc.lock().unwrap() = event.code.clone();
                return Some(confirm(event));
            }
            None
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        response.error.is_none(),
        "introduce failed: {:?}",
        response.error
    );

    let (join_response, mut laptop_ctl) = tokio::time::timeout(Duration::from_secs(10), join)
        .await
        .unwrap()
        .unwrap();
    let join_response = join_response.unwrap();
    assert!(
        join_response.error.is_none(),
        "join failed: {:?}",
        join_response.error
    );
    let (a, b) = (
        laptop_code.lock().unwrap().clone(),
        phone_code.lock().unwrap().clone(),
    );
    assert!(
        a.is_some() && a == b,
        "both sides showed the same code: {a:?} {b:?}"
    );

    // Both now have the person and list each other.
    let s: Status = laptop_ctl
        .request(Request::new(types::STATUS, 0))
        .await
        .unwrap();
    assert!(s.initialised);
    assert_eq!(s.name.as_deref(), Some("Alice"));
    let laptop_devices: Devices = laptop_ctl
        .request(Request::new(types::DEVICES, 0))
        .await
        .unwrap();
    let phone_devices: Devices = phone_ctl
        .request(Request::new(types::DEVICES, 0))
        .await
        .unwrap();
    assert_eq!(
        laptop_devices.devices.len(),
        4,
        "two devices, binding and principal each"
    );
    assert_eq!(phone_devices.devices.len(), 4);
    assert!(
        laptop_devices
            .devices
            .iter()
            .any(|d| d.display_name.as_deref() == Some("phone"))
    );
    assert!(
        phone_devices
            .devices
            .iter()
            .any(|d| d.display_name.as_deref() == Some("laptop"))
    );
    assert_eq!(
        laptop.recognise(phone.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    assert_eq!(
        phone.recognise(laptop.binding().public()).unwrap(),
        Recognition::MyBinding { introducer: true }
    );
    phone.shutdown();
    laptop.shutdown();
}
