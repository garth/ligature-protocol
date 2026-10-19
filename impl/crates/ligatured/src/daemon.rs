//! The daemon: keys, store, endpoint, discovery and the identity state.
//! Milestone 1 decisions LP-25, LP-28, LP-32.
//!
//! One process holds both the binding and the principal key of a personal
//! device. Device keys exist from the first start; the person arrives through
//! `init` (the first device) or the ceremony (every other device). Until then
//! the daemon is uninitialised: it serves its control socket and accepts a
//! pairing, advertises nothing and dials nobody.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use lp_core::facts::{DEVICE_TYPE, DeviceFact, DeviceKind, Registration, Role};
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_core::{Error, PublicKey};
use lp_crypto::signing::SigningKey;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_identity::key_file::KeyFile;
use lp_identity::trust::{Recognition, recognise};
use lp_identity::trust_store::TrustStore;
use lp_transport::manager::{Event, Manager};
use lp_transport::mdns::{InstanceId, Mdns};
use lp_transport::quic::Endpoint;
use tokio::sync::mpsc;

/// What `status` reports.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Whether the daemon has an identity.
    pub initialised: bool,
    /// The daemon's version.
    pub version: String,
    /// The person's master key, once initialised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<PublicKey>,
    /// The person's display name, once initialised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// This device's binding key.
    pub binding: PublicKey,
    /// This device's principal key.
    pub principal: PublicKey,
    /// This device's display name.
    pub device_name: String,
    /// The UDP port the QUIC endpoint is bound to.
    pub port: u16,
    /// Peers with a live, recognised connection.
    #[serde(default)]
    pub peers: Vec<PublicKey>,
}

/// The daemon's state, shared by its tasks and the control channel.
pub struct Daemon {
    locations: Locations,
    keys: Mutex<KeyFile>,
    store: Arc<TrustStore>,
    binding: SigningKey,
    principal: SigningKey,
    device_name: String,
    manager: Manager,
    mdns: Mutex<Option<Mdns>>,
    instance_id: InstanceId,
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Daemon({})", self.binding.public())
    }
}

/// The platform this build runs on, as the device record states it.
#[must_use]
pub const fn platform() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "other"
    }
}

/// The host name without a domain, or `device` if the system has none.
#[must_use]
pub fn host_name() -> String {
    let name = gethostname::gethostname().to_string_lossy().into_owned();
    let short = name.split('.').next().unwrap_or("").trim();
    if short.is_empty() {
        "device".to_owned()
    } else {
        short.to_owned()
    }
}

impl Daemon {
    /// Opens or creates everything under `locations`, binds the QUIC endpoint
    /// on `bind` (port 0 for an assigned port) and starts accepting.
    /// `device_name` overrides the host name for a device that has no record
    /// yet. Returns the daemon and the stream of connection events.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`], [`Error::Store`] or [`Error::InvalidKey`].
    pub fn open(
        locations: Locations,
        bind: SocketAddr,
        device_name: Option<&str>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<Event>), Error> {
        let store = Arc::new(TrustStore::open(&locations)?);
        let mut keys = KeyFile::load(&locations.keys())?;
        let (binding, principal) = match (keys.binding_key()?, keys.principal_key()?) {
            (Some(b), Some(p)) => (b, p),
            _ => {
                let b = SigningKey::generate()?;
                let p = SigningKey::generate()?;
                keys.set_device_keys(&b, &p);
                keys.save(&locations.keys())?;
                tracing::info!(binding = %b.public(), principal = %p.public(), "created device keys");
                (b, p)
            }
        };
        let device_name = match stored_device_name(&store, binding.public())? {
            Some(name) => name,
            None => device_name.map_or_else(host_name, str::to_owned),
        };
        let endpoint = Endpoint::bind(bind, &binding)?;
        let (manager, events) = Manager::start(endpoint);
        let daemon = Arc::new(Self {
            locations,
            keys: Mutex::new(keys),
            store,
            binding,
            principal,
            device_name,
            manager,
            mdns: Mutex::new(None),
            instance_id: InstanceId::random()?,
        });
        if daemon.is_initialised() {
            daemon.start_advertising()?;
        }
        Ok((daemon, events))
    }

    /// Whether this device has a person.
    #[must_use]
    pub fn is_initialised(&self) -> bool {
        self.lock_keys().is_initialised()
    }

    /// The person's master key, if initialised.
    #[must_use]
    pub fn person(&self) -> Option<PublicKey> {
        self.lock_keys().person
    }

    /// The device's binding key.
    #[must_use]
    pub const fn binding(&self) -> &SigningKey {
        &self.binding
    }

    /// The device's principal key.
    #[must_use]
    pub const fn principal(&self) -> &SigningKey {
        &self.principal
    }

    /// The device's display name.
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The trust store.
    #[must_use]
    pub fn store(&self) -> &Arc<TrustStore> {
        &self.store
    }

    /// The connection manager.
    #[must_use]
    pub const fn manager(&self) -> &Manager {
        &self.manager
    }

    /// Where the daemon keeps its files.
    #[must_use]
    pub const fn locations(&self) -> &Locations {
        &self.locations
    }

    /// The self-signing key, if this device is an introducer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if the key file is damaged.
    pub fn self_signing(&self) -> Result<Option<SigningKey>, Error> {
        self.lock_keys().self_signing_key()
    }

    /// Creates the person on this first device: three person keys, the person
    /// fact, roles over this device's keys (an introducer), and the device
    /// records. Starts advertising.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AlreadyInitialised`], or a store or key file error.
    pub fn init(&self, person_name: &str, device_name: Option<&str>) -> Result<PublicKey, Error> {
        if self.is_initialised() {
            return Err(Error::AlreadyInitialised);
        }
        let person = PersonKeys::generate()?;
        let now = Timestamp::now();
        let name = device_name.unwrap_or(&self.device_name);
        let person_fact = facts::person_fact(&person, person_name, 1, now)?;
        let binding_role =
            facts::role_fact(&person, Role::Binding, *self.binding.public(), true, now)?;
        let principal_role = facts::role_fact(
            &person,
            Role::Principal,
            *self.principal.public(),
            false,
            now,
        )?;
        let (binding_rec, principal_rec) =
            self.device_records(name, Some(*person.person()), true, now)?;
        for env in [
            &person_fact,
            &binding_role,
            &principal_role,
            &binding_rec,
            &principal_rec,
        ] {
            self.store.insert(env)?;
        }
        {
            let mut keys = self.lock_keys();
            keys.set_person(&person);
            keys.save(&self.locations.keys())?;
        }
        tracing::info!(person = %person.person(), name = person_name, "initialised");
        self.start_advertising()?;
        Ok(*person.person())
    }

    /// Self-signed device records for this device.
    fn device_records(
        &self,
        name: &str,
        person: Option<PublicKey>,
        introducer: bool,
        now: Timestamp,
    ) -> Result<(lp_core::envelope::Envelope, lp_core::envelope::Envelope), Error> {
        let binding = DeviceFact {
            key: *self.binding.public(),
            kind: DeviceKind::Binding,
            display_name: name.to_owned(),
            platform: platform().to_owned(),
            registration: None,
            person,
            principal: Some(*self.principal.public()),
            introducer,
            created_at: now,
        };
        let principal = DeviceFact {
            key: *self.principal.public(),
            kind: DeviceKind::Principal,
            display_name: name.to_owned(),
            platform: platform().to_owned(),
            registration: Some(Registration::Personal),
            person: None,
            principal: None,
            introducer: false,
            created_at: now,
        };
        Ok((
            facts::device_fact(&self.binding, &binding)?,
            facts::device_fact(&self.principal, &principal)?,
        ))
    }

    /// Starts (or restarts) the mDNS advertisement for the endpoint's port.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if the responder cannot start.
    pub fn start_advertising(&self) -> Result<(), Error> {
        let port = self.manager.endpoint().local_addr()?.port();
        let mut slot = self
            .mdns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(Mdns::advertise(self.instance_id.clone(), port)?);
        }
        Ok(())
    }

    /// The mDNS responder, once advertising.
    pub fn with_mdns<R>(&self, f: impl FnOnce(&Mdns) -> R) -> Option<R> {
        self.mdns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(f)
    }

    /// What a connected peer is to this device.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotInitialised`] before there is a person.
    pub fn recognise(&self, peer: &PublicKey) -> Result<Recognition, Error> {
        let owner = self.person().ok_or(Error::NotInitialised)?;
        recognise(&self.store, &owner, peer)
    }

    /// The current status.
    #[must_use]
    pub fn status(&self) -> Status {
        let person = self.person();
        let name = person.and_then(|p| person_name(&self.store, &p));
        Status {
            initialised: person.is_some(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            person,
            name,
            binding: *self.binding.public(),
            principal: *self.principal.public(),
            device_name: self.device_name.clone(),
            port: self
                .manager
                .endpoint()
                .local_addr()
                .map(|a| a.port())
                .unwrap_or(0),
            peers: self.manager.peers(),
        }
    }

    /// Mutates the key file and saves it.
    ///
    /// # Errors
    ///
    /// Returns the save error.
    pub fn update_keys(&self, f: impl FnOnce(&mut KeyFile)) -> Result<(), Error> {
        let mut keys = self.lock_keys();
        f(&mut keys);
        keys.save(&self.locations.keys())
    }

    /// Stops advertising and closes every connection.
    pub fn shutdown(&self) {
        if let Some(m) = self
            .mdns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            m.shutdown();
        }
        self.manager.shutdown();
    }

    fn lock_keys(&self) -> std::sync::MutexGuard<'_, KeyFile> {
        self.keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn stored_device_name(store: &TrustStore, binding: &PublicKey) -> Result<Option<String>, Error> {
    let mut newest: Option<DeviceFact> = None;
    for stored in store.about(binding)? {
        if stored.envelope.fact_type.as_str() != DEVICE_TYPE {
            continue;
        }
        if let Ok(fact) = facts::verify_device(&stored.envelope) {
            if newest
                .as_ref()
                .is_none_or(|n| fact.created_at > n.created_at)
            {
                newest = Some(fact);
            }
        }
    }
    Ok(newest.map(|f| f.display_name))
}

/// The person's display name from the newest person fact in the store.
#[must_use]
pub fn person_name(store: &TrustStore, person: &PublicKey) -> Option<String> {
    let mut best: Option<lp_core::facts::PersonFact> = None;
    for stored in store.about(person).ok()? {
        if stored.envelope.fact_type.as_str() != lp_core::facts::PERSON_TYPE {
            continue;
        }
        if let Ok(fact) = facts::verify_person(&stored.envelope) {
            if best.as_ref().is_none_or(|b| fact.version > b.version) {
                best = Some(fact);
            }
        }
    }
    best.map(|f| f.display_name)
}
