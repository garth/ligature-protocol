//! Phase B end to end on one machine: three daemons advertise over mDNS, one
//! browses, dials every supported instance, authenticates each with raw
//! public keys, keeps the one it trusts and exchanges the gossip stream
//! header with it, and drops the stranger and backs off from its `id`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lp_core::PublicKey;
use lp_core::frame::{GossipHave, StreamHeader};
use lp_crypto::signing::SigningKey;
use lp_transport::manager::Manager;
use lp_transport::mdns::{DialPolicy, Discovered, InstanceId, Mdns};
use lp_transport::quic::Endpoint;

struct Daemon {
    key: SigningKey,
    manager: Manager,
    mdns: Mdns,
}

fn daemon() -> Daemon {
    let key = SigningKey::generate().unwrap();
    // Bound to every interface so the addresses mDNS advertises are dialable.
    let ep = Endpoint::bind("0.0.0.0:0".parse().unwrap(), &key).unwrap();
    let port = ep.local_addr().unwrap().port();
    let (manager, _events) = Manager::start(ep);
    manager.on_stream(
        StreamHeader::GOSSIP,
        Arc::new(|_conn, mut stream| {
            Box::pin(async move {
                // Answer the header with an empty have-list, then finish.
                let _ = stream.send(&GossipHave::new(vec![])).await;
                stream.finish();
            })
        }),
    );
    let mdns = Mdns::advertise(InstanceId::random().unwrap(), port).unwrap();
    Daemon { key, manager, mdns }
}

#[tokio::test]
async fn discover_dial_authenticate_and_back_off_strangers() {
    let a = daemon();
    let b = daemon();
    let c = daemon();
    let trusted: HashSet<PublicKey> = [*b.key.public()].into_iter().collect();

    let mut browser = a.mdns.browse().unwrap();
    let mut policy = DialPolicy::new(Duration::from_secs(60), 4);
    let mut trusted_peer: Option<PublicKey> = None;
    let mut stranger_id: Option<InstanceId> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);

    while trusted_peer.is_none() || stranger_id.is_none() {
        let event = tokio::time::timeout_at(deadline, browser.recv())
            .await
            .expect("both peers within 20 s")
            .expect("browser alive");
        let Discovered::Found { id, addrs } = event else {
            continue;
        };
        if id != *b.mdns.id() && id != *c.mdns.id() {
            continue; // Something else on this LAN.
        }
        // The endpoints are IPv4 here; a first resolution may carry only an
        // IPv6 address, in which case the next update brings the rest.
        let Some(addr) = addrs.iter().copied().find(|a| a.is_ipv4()) else {
            continue;
        };
        if !policy.try_begin(&id, Instant::now()) {
            continue;
        }
        let conn = a
            .manager
            .connect(addr)
            .await
            .expect("handshake with an advertised peer");
        if trusted.contains(conn.peer()) {
            // Recognised: the dialer opens the gossip stream first.
            let mut gossip = a
                .manager
                .open(conn.peer(), StreamHeader::GOSSIP)
                .await
                .unwrap();
            let reply: GossipHave = tokio::time::timeout(Duration::from_secs(5), gossip.recv())
                .await
                .unwrap()
                .unwrap()
                .expect("peer answered on the gossip stream");
            assert!(reply.facts.is_empty());
            policy.finish(&id, true, Instant::now());
            trusted_peer = Some(*conn.peer());
        } else {
            // A stranger: authenticated, not recognised, offered nothing.
            assert_eq!(conn.peer(), c.key.public());
            a.manager.drop_peer(conn.peer());
            policy.finish(&id, false, Instant::now());
            assert!(
                !policy.try_begin(&id, Instant::now()),
                "stranger is backed off"
            );
            stranger_id = Some(id);
        }
    }

    assert_eq!(trusted_peer, Some(*b.key.public()));
    assert_eq!(
        a.manager.peers(),
        vec![*b.key.public()],
        "only the trusted peer stays connected"
    );
    assert!(a.manager.peer(c.key.public()).is_none());
    assert!(stranger_id.is_some());

    a.manager.shutdown();
    b.manager.shutdown();
    c.manager.shutdown();
    a.mdns.shutdown();
    b.mdns.shutdown();
    c.mdns.shutdown();
}
