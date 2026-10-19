//! Two daemons on one machine find each other over mDNS, each with its own
//! `id` and port, and the record carries nothing identifying.

use std::time::Duration;

use lp_transport::mdns::{Discovered, InstanceId, Mdns};

async fn wait_for(rx: &mut tokio::sync::mpsc::Receiver<Discovered>, want: &InstanceId, port: u16) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("discovery within 15 s")
            .expect("browser alive");
        if let Discovered::Found { id, addrs } = ev {
            if id == *want {
                assert!(!addrs.is_empty(), "resolved with addresses");
                assert!(addrs.iter().all(|a| a.port() == port), "{addrs:?}");
                return;
            }
        }
    }
}

#[tokio::test]
async fn two_instances_discover_each_other() {
    let a_id = InstanceId::random().unwrap();
    let b_id = InstanceId::random().unwrap();
    let a = Mdns::advertise(a_id.clone(), 40001).unwrap();
    let b = Mdns::advertise(b_id.clone(), 40002).unwrap();
    let mut a_rx = a.browse().unwrap();
    let mut b_rx = b.browse().unwrap();

    wait_for(&mut a_rx, &b_id, 40002).await;
    wait_for(&mut b_rx, &a_id, 40001).await;

    b.shutdown();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let ev = tokio::time::timeout_at(deadline, a_rx.recv())
            .await
            .expect("removal within 15 s")
            .expect("browser alive");
        if ev == (Discovered::Gone { id: b_id.clone() }) {
            break;
        }
    }
    a.shutdown();
}
