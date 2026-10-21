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

/// Set to `1` to use the in-memory fake clipboard instead of the platform's:
/// for the loopback end-to-end run and for machines without a compositor.
pub const FAKE_CLIPBOARD_ENV: &str = "LIGATURE_FAKE_CLIPBOARD";

/// The address the QUIC endpoint binds: every interface, assigned port.
#[must_use]
pub fn default_bind() -> SocketAddr {
    bind_on(None)
}

/// Environment variable naming the UDP port, the default for `--port`.
pub const PORT_ENV: &str = "LIGATURE_PORT";

/// All IPv4 interfaces on `port`, or on one the operating system assigns.
/// A fixed port is for machines whose firewall must allow peers to dial in
/// (LP-81); the advertised SRV record carries whichever port was bound.
#[must_use]
pub fn bind_on(port: Option<u16>) -> SocketAddr {
    SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, port.unwrap_or(0)))
}

/// Opens the daemon and runs it until the control socket fails.
///
/// # Errors
///
/// Returns the error that stopped it.
pub async fn run(
    locations: Locations,
    device_name: Option<&str>,
    port: Option<u16>,
) -> Result<(), Error> {
    let (daemon, events) = Daemon::open(locations.clone(), bind_on(port), device_name)?;
    tracing::info!(
        binding = %daemon.binding().public(),
        initialised = daemon.is_initialised(),
        port = daemon.status().port,
        "ligatured running"
    );
    let control = control::serve(daemon.clone(), &locations.control_socket())?;
    let clipboard: Option<Arc<dyn crate::clipboard::Clipboard>> =
        if std::env::var_os(FAKE_CLIPBOARD_ENV).is_some_and(|v| v == "1") {
            tracing::warn!("using the in-memory fake clipboard ({FAKE_CLIPBOARD_ENV}=1)");
            Some(crate::clipboard::FakeClipboard::new())
        } else {
            crate::clipboard::platform_clipboard()
        };
    match clipboard {
        Some(clipboard) => {
            daemon.attach_clipboard(clipboard);
        }
        None => tracing::warn!("no clipboard implementation on this platform"),
    }

    spawn_connection_events(daemon.clone(), events);
    spawn_discovery(daemon.clone());

    control
        .await
        .map_err(|e| Error::Io(format!("control task: {e}")))?;
    daemon.shutdown();
    Ok(())
}

/// Handles connection events for `daemon` in a task: a recognised peer gets
/// gossip and the manifest exchange, a stranger is kept only while a ceremony
/// can use it, a departed peer has its capabilities withdrawn. Runs until the
/// manager closes the channel.
pub fn spawn_connection_events(
    daemon: Arc<Daemon>,
    mut events: tokio::sync::mpsc::Receiver<Event>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                Event::Connected(conn) => match daemon.recognise(conn.peer()) {
                    Ok(r) if r.is_trusted() => {
                        tracing::info!(peer = %conn.peer(), ?r, "peer connected");
                        let d = daemon.clone();
                        tokio::spawn(async move { d.on_trusted(&conn).await });
                    }
                    Ok(r) => {
                        if daemon.accepts_strangers() {
                            tracing::info!(peer = %conn.peer(), ?r, "unrecognised peer; pairing only");
                        } else {
                            tracing::info!(peer = %conn.peer(), ?r, "unrecognised peer; dropping");
                            daemon.manager().drop_peer(conn.peer());
                        }
                    }
                    Err(Error::NotInitialised) => {
                        tracing::debug!(peer = %conn.peer(), "not initialised; connection kept for pairing");
                    }
                    Err(e) => tracing::warn!(%e, "recognition failed"),
                },
                Event::Disconnected(peer) => {
                    tracing::info!(%peer, "peer disconnected");
                    daemon.on_disconnected(&peer);
                }
            }
        }
    })
}

/// Browses mDNS for `daemon` in a task once it is initialised and dials every
/// instance found; the handshake and the trust store decide what to keep.
pub fn spawn_discovery(daemon: Arc<Daemon>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move { discovery_loop(daemon).await })
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
