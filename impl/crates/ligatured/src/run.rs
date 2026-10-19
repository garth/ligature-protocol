//! Runs the daemon: control socket, connection events, discovery and dialling.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lp_core::Error;
use lp_core::paths::Locations;
use lp_transport::manager::Event;
use lp_transport::mdns::{DialPolicy, Discovered};

use crate::control;
use crate::daemon::Daemon;

/// The address the QUIC endpoint binds: every interface, assigned port.
#[must_use]
pub fn default_bind() -> SocketAddr {
    "0.0.0.0:0".parse().expect("literal")
}

/// Opens the daemon and runs it until the control socket fails.
///
/// # Errors
///
/// Returns the error that stopped it.
pub async fn run(locations: Locations, device_name: Option<&str>) -> Result<(), Error> {
    let (daemon, mut events) = Daemon::open(locations.clone(), default_bind(), device_name)?;
    tracing::info!(
        binding = %daemon.binding().public(),
        initialised = daemon.is_initialised(),
        port = daemon.status().port,
        "ligatured running"
    );
    let control = control::serve(daemon.clone(), &locations.control_socket())?;

    // Connection events: recognise, keep or drop.
    let for_events = daemon.clone();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                Event::Connected(conn) => match for_events.recognise(conn.peer()) {
                    Ok(r) if r.is_trusted() => {
                        tracing::info!(peer = %conn.peer(), ?r, "peer connected");
                    }
                    Ok(r) => {
                        if for_events.accepts_strangers() {
                            tracing::info!(peer = %conn.peer(), ?r, "unrecognised peer; pairing only");
                        } else {
                            tracing::info!(peer = %conn.peer(), ?r, "unrecognised peer; dropping");
                            for_events.manager().drop_peer(conn.peer());
                        }
                    }
                    Err(Error::NotInitialised) => {
                        tracing::debug!(peer = %conn.peer(), "not initialised; connection kept for pairing");
                    }
                    Err(e) => tracing::warn!(%e, "recognition failed"),
                },
                Event::Disconnected(peer) => tracing::info!(%peer, "peer disconnected"),
            }
        }
    });

    // Discovery: dial every supported instance; the handshake decides.
    let for_discovery = daemon.clone();
    tokio::spawn(async move { discovery_loop(for_discovery).await });

    control
        .await
        .map_err(|e| Error::Io(format!("control task: {e}")))?;
    daemon.shutdown();
    Ok(())
}

async fn discovery_loop(daemon: Arc<Daemon>) {
    // Wait until the daemon is initialised and advertising.
    loop {
        if daemon.is_initialised() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let Some(Ok(mut found)) = daemon.with_mdns(|m| m.browse()) else {
        tracing::error!("cannot browse mDNS");
        return;
    };
    let mut policy = DialPolicy::new(Duration::from_secs(30), 4);
    while let Some(event) = found.recv().await {
        match event {
            Discovered::Found { id, addrs } => {
                let Some(addr) = addrs.iter().copied().find(|a| a.is_ipv4()) else {
                    continue;
                };
                if !policy.try_begin(&id, Instant::now()) {
                    continue;
                }
                let outcome = daemon.manager().connect(addr).await;
                let authenticated = match outcome {
                    Ok(conn) => {
                        let trusted = daemon
                            .recognise(conn.peer())
                            .map(|r| r.is_trusted())
                            .unwrap_or(false);
                        if !trusted {
                            tracing::debug!(peer = %conn.peer(), "dialled a stranger; dropping");
                            daemon.manager().drop_peer(conn.peer());
                        }
                        trusted
                    }
                    Err(e) => {
                        tracing::debug!(%addr, %e, "dial failed");
                        false
                    }
                };
                policy.finish(&id, authenticated, Instant::now());
            }
            Discovered::Gone { id } => policy.forget(&id),
        }
    }
}
