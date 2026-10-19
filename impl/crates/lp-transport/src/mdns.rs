//! mDNS/DNS-SD local discovery. Spec chapter 04 section 4.1; ADR 0037.
//!
//! A daemon advertises one `_ligature._udp` instance named `lp-<id>` on host
//! `lp-<id>.local` with TXT keys `v` and `id` and nothing else, where `id` is
//! eight random bytes drawn at start. It browses for the same type and reports
//! every instance whose version it supports. `id` is a hint for telling
//! instances apart and backing off after a failed authentication; recognition
//! is the raw-public-key handshake and nothing before it.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use lp_core::Error;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;

/// The DNS-SD service type.
pub const SERVICE_TYPE: &str = "_ligature._udp.local.";
/// The protocol version advertised in `v`, the same number as the ALPN token.
pub const VERSION: &str = "1";

/// The per-start identifier: 8 random bytes as 16 lowercase hex characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstanceId(String);

impl InstanceId {
    /// Draws a fresh identifier from operating-system randomness.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Randomness`] if the operating system refuses.
    pub fn random() -> Result<Self, Error> {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).map_err(|e| Error::Randomness(e.to_string()))?;
        Ok(Self(bytes.iter().map(|b| format!("{b:02x}")).collect()))
    }

    /// Parses a received identifier: exactly 16 lowercase hex characters.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        (s.len() == 16
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| Self(s.to_owned()))
    }

    /// The hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The instance name, `lp-<id>`.
    #[must_use]
    pub fn instance_name(&self) -> String {
        format!("lp-{}", self.0)
    }

    /// The host name, `lp-<id>.local.`.
    #[must_use]
    pub fn host_name(&self) -> String {
        format!("lp-{}.local.", self.0)
    }
}

impl std::fmt::Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Builds the record a daemon advertises: the only place the record's shape
/// is decided.
///
/// # Errors
///
/// Returns [`Error::Transport`] if `mdns-sd` refuses the record.
pub fn service_info(id: &InstanceId, port: u16) -> Result<ServiceInfo, Error> {
    let mut txt = HashMap::new();
    txt.insert("v".to_owned(), VERSION.to_owned());
    txt.insert("id".to_owned(), id.as_str().to_owned());
    ServiceInfo::new(
        SERVICE_TYPE,
        &id.instance_name(),
        &id.host_name(),
        (),
        port,
        txt,
    )
    .map(ServiceInfo::enable_addr_auto)
    .map_err(|e| Error::Transport(format!("mdns record: {e}")))
}

/// Something the browser saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovered {
    /// An instance with a supported version, with the addresses and port it
    /// advertised. Repeated as addresses change.
    Found {
        /// Its `id`.
        id: InstanceId,
        /// Where to dial.
        addrs: Vec<SocketAddr>,
    },
    /// An instance went away.
    Gone {
        /// Its `id`.
        id: InstanceId,
    },
}

/// mDNS advertiser and browser for one daemon.
pub struct Mdns {
    daemon: ServiceDaemon,
    id: InstanceId,
    fullname: String,
}

impl std::fmt::Debug for Mdns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Mdns({})", self.id)
    }
}

impl Mdns {
    /// Starts the mDNS responder and advertises `port` under `id`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if the responder cannot start or register.
    pub fn advertise(id: InstanceId, port: u16) -> Result<Self, Error> {
        let daemon =
            ServiceDaemon::new().map_err(|e| Error::Transport(format!("mdns start: {e}")))?;
        let info = service_info(&id, port)?;
        let fullname = info.get_fullname().to_owned();
        daemon
            .register(info)
            .map_err(|e| Error::Transport(format!("mdns register: {e}")))?;
        tracing::info!(%id, port, "advertising on mDNS");
        Ok(Self {
            daemon,
            id,
            fullname,
        })
    }

    /// This daemon's identifier.
    #[must_use]
    pub const fn id(&self) -> &InstanceId {
        &self.id
    }

    /// Browses for other instances. Our own is filtered out by `id`; anything
    /// without a supported `v` or a well-formed `id` is ignored.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if browsing cannot start.
    pub fn browse(&self) -> Result<mpsc::Receiver<Discovered>, Error> {
        let events = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| Error::Transport(format!("mdns browse: {e}")))?;
        let (tx, rx) = mpsc::channel(64);
        let me = self.id.clone();
        tokio::spawn(async move {
            let mut names: HashMap<String, InstanceId> = HashMap::new();
            while let Ok(event) = events.recv_async().await {
                let out = match event {
                    ServiceEvent::ServiceResolved(service) => {
                        let Some(id) = service
                            .get_property_val_str("id")
                            .and_then(InstanceId::parse)
                        else {
                            continue;
                        };
                        if service.get_property_val_str("v") != Some(VERSION) || id == me {
                            continue;
                        }
                        let port = service.get_port();
                        let mut addrs: Vec<SocketAddr> = service
                            .get_addresses()
                            .iter()
                            .map(|a| SocketAddr::new(a.to_ip_addr(), port))
                            .collect();
                        addrs.sort_by_key(|a| (!matches!(a.ip(), IpAddr::V4(_)), a.to_string()));
                        names.insert(service.get_fullname().to_owned(), id.clone());
                        Discovered::Found { id, addrs }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => match names.remove(&fullname) {
                        Some(id) => Discovered::Gone { id },
                        None => continue,
                    },
                    _ => continue,
                };
                if tx.send(out).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }

    /// Withdraws the advertisement and stops the responder.
    pub fn shutdown(self) {
        if let Ok(rx) = self.daemon.unregister(&self.fullname) {
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }
        let _ = self.daemon.shutdown();
    }
}

/// Remembers which instances recently failed authentication so the dialler
/// does not hammer them, and caps how many dials are in flight.
#[derive(Debug)]
pub struct DialPolicy {
    backoff: Duration,
    max_in_flight: usize,
    failed_at: HashMap<InstanceId, Instant>,
    in_flight: HashSet<InstanceId>,
}

impl DialPolicy {
    /// A policy with the given back-off after a failure and dial cap.
    #[must_use]
    pub fn new(backoff: Duration, max_in_flight: usize) -> Self {
        Self {
            backoff,
            max_in_flight,
            failed_at: HashMap::new(),
            in_flight: HashSet::new(),
        }
    }

    /// Whether `id` may be dialled now; if so, records the dial as in flight.
    pub fn try_begin(&mut self, id: &InstanceId, now: Instant) -> bool {
        if self.in_flight.contains(id) || self.in_flight.len() >= self.max_in_flight {
            return false;
        }
        if let Some(failed) = self.failed_at.get(id) {
            if now.duration_since(*failed) < self.backoff {
                return false;
            }
            self.failed_at.remove(id);
        }
        self.in_flight.insert(id.clone());
        true
    }

    /// The dial of `id` ended; `authenticated` says whether the handshake
    /// completed with a key we were prepared to talk to.
    pub fn finish(&mut self, id: &InstanceId, authenticated: bool, now: Instant) {
        self.in_flight.remove(id);
        if authenticated {
            self.failed_at.remove(id);
        } else {
            self.failed_at.insert(id.clone(), now);
        }
    }

    /// Forgets an instance that went away.
    pub fn forget(&mut self, id: &InstanceId) {
        self.in_flight.remove(id);
        self.failed_at.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_sixteen_hex_characters() {
        let id = InstanceId::random().unwrap();
        assert_eq!(id.as_str().len(), 16);
        assert_eq!(InstanceId::parse(id.as_str()), Some(id.clone()));
        assert_eq!(id.instance_name(), format!("lp-{id}"));
        assert_eq!(id.host_name(), format!("lp-{id}.local."));
        assert!(
            InstanceId::parse("ABCDEF0123456789").is_none(),
            "upper case"
        );
        assert!(InstanceId::parse("abc").is_none());
        assert!(InstanceId::parse("").is_none());
    }

    #[test]
    fn the_record_carries_only_v_and_id_and_nothing_identifying() {
        let id = InstanceId::parse("0123456789abcdef").unwrap();
        let info = service_info(&id, 4242).unwrap();
        assert_eq!(info.get_type(), SERVICE_TYPE);
        assert_eq!(
            info.get_fullname(),
            "lp-0123456789abcdef._ligature._udp.local."
        );
        assert_eq!(info.get_hostname(), "lp-0123456789abcdef.local.");
        assert_eq!(info.get_port(), 4242);
        let props = info.get_properties();
        assert_eq!(props.len(), 2);
        assert_eq!(props.get_property_val_str("v"), Some("1"));
        assert_eq!(props.get_property_val_str("id"), Some("0123456789abcdef"));
        assert!(info.is_addr_auto());
    }

    #[test]
    fn the_dial_policy_backs_off_and_caps() {
        let a = InstanceId::parse("aaaaaaaaaaaaaaaa").unwrap();
        let b = InstanceId::parse("bbbbbbbbbbbbbbbb").unwrap();
        let c = InstanceId::parse("cccccccccccccccc").unwrap();
        let mut p = DialPolicy::new(Duration::from_secs(30), 2);
        let t0 = Instant::now();
        assert!(p.try_begin(&a, t0));
        assert!(!p.try_begin(&a, t0), "already in flight");
        assert!(p.try_begin(&b, t0));
        assert!(!p.try_begin(&c, t0), "cap of two");
        p.finish(&a, false, t0);
        assert!(!p.try_begin(&a, t0 + Duration::from_secs(5)), "backing off");
        assert!(p.try_begin(&c, t0));
        p.finish(&b, true, t0);
        assert!(
            p.try_begin(&a, t0 + Duration::from_secs(31)),
            "back-off expired"
        );
        p.forget(&a);
        assert!(p.try_begin(&a, t0 + Duration::from_secs(31)));
    }
}
