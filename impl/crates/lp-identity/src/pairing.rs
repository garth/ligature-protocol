//! The proximity ceremony. Spec chapter 02 sections 4 to 6; ADR 0029.
//!
//! The newcomer shows an offer (its keys, where to dial, a one-time code) as a
//! QR code or text; an introducer dials it over raw-public-key QUIC, checks the
//! peer key is the one in the offer, and both derive the six-digit code from
//! the TLS exporter. The users compare the codes. Only after both have
//! confirmed does the introducer send the person fact, the role facts over
//! the newcomer's keys, the self-signing key if the newcomer becomes an
//! introducer, and current gossip. The newcomer refuses anything that arrives
//! before its own confirmation.
//!
//! This module is the state machine on each side and the messages between
//! them (`schemas/pairing.schema.json`); it moves no bytes itself. The daemon
//! pumps a framed stream on the `pairing` purpose through it and supplies the
//! exporter output.

use std::net::SocketAddr;

use lp_core::envelope::Envelope;
use lp_core::facts::{DeviceFact, DeviceKind, Registration, Role};
use lp_core::wire::{Bytes, Timestamp, TypeName};
use lp_core::{Error, PublicKey};
use lp_crypto::hash::sha256;
use lp_crypto::signing::SigningKey;
use serde::{Deserialize, Serialize};

use crate::cross_signing::PersonKeys;
use crate::facts;

/// The stream purpose of the ceremony.
pub const PURPOSE: &str = "pairing";
/// The TLS exporter label (ADR 0029).
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-ligatureprotocol-pairing-sas";
/// Exporter output length.
pub const EXPORTER_LEN: usize = 32;

/// Type names of the ceremony messages.
pub mod types {
    /// Introducer to newcomer: here is the one-time code from your offer.
    pub const HELLO: &str = "org.ligatureprotocol.pair.hello";
    /// Newcomer to introducer: my device records.
    pub const READY: &str = "org.ligatureprotocol.pair.ready";
    /// Either side: my user confirmed the codes match.
    pub const CONFIRMED: &str = "org.ligatureprotocol.pair.confirmed";
    /// Introducer to newcomer: the facts that make you one of us.
    pub const GRANT: &str = "org.ligatureprotocol.pair.grant";
    /// Newcomer to introducer: stored, done.
    pub const DONE: &str = "org.ligatureprotocol.pair.done";
    /// Either side: stop.
    pub const ABORT: &str = "org.ligatureprotocol.pair.abort";
}

/// What the newcomer shows: rendered as a QR code and printed as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The newcomer's binding key.
    pub binding: PublicKey,
    /// The newcomer's principal key.
    pub principal: PublicKey,
    /// Where the introducer can dial it.
    pub addrs: Vec<SocketAddr>,
    /// One-time code: proves the introducer saw this offer. 16 hex characters.
    pub code: String,
}

impl Offer {
    const SCHEME: &'static str = "ligature:pair/1?";

    /// The text form, `ligature:pair/1?b=…&p=…&a=…&c=…`.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut s = format!("{}b={}&p={}", Self::SCHEME, self.binding, self.principal);
        for a in &self.addrs {
            s.push_str("&a=");
            s.push_str(&a.to_string());
        }
        s.push_str("&c=");
        s.push_str(&self.code);
        s
    }

    /// Parses the text form.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if a field is missing or malformed.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let bad = |what: &str| Error::Decode(format!("offer: {what}"));
        let query = text
            .strip_prefix(Self::SCHEME)
            .ok_or_else(|| bad("not a ligature pairing offer"))?;
        let (mut binding, mut principal, mut code) = (None, None, None);
        let mut addrs = Vec::new();
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').ok_or_else(|| bad("malformed field"))?;
            match k {
                "b" => binding = Some(v.parse().map_err(|_| bad("binding key"))?),
                "p" => principal = Some(v.parse().map_err(|_| bad("principal key"))?),
                "a" => addrs.push(v.parse().map_err(|_| bad("address"))?),
                "c" => code = Some(v.to_owned()),
                _ => {}
            }
        }
        let code: String = code.ok_or_else(|| bad("no code"))?;
        if code.len() != 16 || !code.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad("code must be 16 hex characters"));
        }
        if addrs.is_empty() {
            return Err(bad("no address"));
        }
        Ok(Self {
            binding: binding.ok_or_else(|| bad("no binding key"))?,
            principal: principal.ok_or_else(|| bad("no principal key"))?,
            addrs,
            code,
        })
    }
}

/// Introducer to newcomer, first on the stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Always [`types::HELLO`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// The one-time code from the offer.
    pub code: String,
}

/// Newcomer to introducer, in answer to a valid hello.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    /// Always [`types::READY`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// The newcomer's device records, self-signed: binding then principal.
    pub devices: Vec<Envelope>,
}

/// Either side: my user confirmed the codes match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confirmed {
    /// Always [`types::CONFIRMED`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
}

/// Introducer to newcomer, after both confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    /// Always [`types::GRANT`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// The introducer's person fact.
    pub person: Envelope,
    /// Role facts over the newcomer's keys: `binding` and `principal`.
    pub roles: Vec<Envelope>,
    /// The self-signing key's seed, if the newcomer becomes an introducer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_signing: Option<Bytes>,
    /// Current gossip: everything else the introducer holds.
    #[serde(default)]
    pub gossip: Vec<Envelope>,
}

/// Newcomer to introducer: stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    /// Always [`types::DONE`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
}

/// Either side: stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Abort {
    /// Always [`types::ABORT`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// Why, for the log.
    pub reason: String,
}

impl Abort {
    /// An abort message.
    #[must_use]
    pub fn new(reason: &str) -> Self {
        Self {
            message_type: TypeName::new(types::ABORT),
            reason: reason.to_owned(),
        }
    }
}

/// The exporter context of ADR 0029: SHA-256 of the newcomer's key followed
/// by the introducer's key. Both are the keys the TLS handshake authenticated,
/// which for the newcomer is its binding key.
#[must_use]
pub fn exporter_context(newcomer: &PublicKey, introducer: &PublicKey) -> [u8; 32] {
    let mut m = Vec::with_capacity(64);
    m.extend_from_slice(newcomer.as_bytes());
    m.extend_from_slice(introducer.as_bytes());
    sha256(&m)
}

/// The six-digit code from the exporter output: the first 20 bits, as six
/// decimal digits (ADR 0029).
///
/// # Errors
///
/// Returns [`Error::Decode`] if the output is shorter than 4 bytes.
pub fn short_code(exporter: &[u8]) -> Result<String, Error> {
    let first: [u8; 4] = exporter
        .get(..4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| Error::Decode("exporter output too short".to_owned()))?;
    let bits = u32::from_be_bytes(first) >> 12;
    Ok(format!("{:06}", bits % 1_000_000))
}

/// Where the newcomer is in the ceremony.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewcomerState {
    /// Showing the offer, waiting for an introducer's hello.
    Offering,
    /// Hello accepted; codes can be derived and shown.
    Greeted,
    /// The user confirmed; waiting for the grant.
    Confirmed,
    /// The grant was accepted.
    Done,
    /// Stopped.
    Aborted,
}

/// What a newcomer ends the ceremony with, for the daemon to persist.
#[derive(Debug, Clone)]
pub struct Joined {
    /// The introducer's person: now mine.
    pub person: Envelope,
    /// The roles over my keys.
    pub roles: Vec<Envelope>,
    /// My device records, as sent.
    pub devices: Vec<Envelope>,
    /// The self-signing key, if I am an introducer.
    pub self_signing: Option<SigningKey>,
    /// Everything else the introducer knew.
    pub gossip: Vec<Envelope>,
}

/// The newcomer's side.
#[derive(Debug)]
pub struct Newcomer {
    binding: SigningKey,
    principal: SigningKey,
    code: String,
    devices: Vec<Envelope>,
    introducer: Option<PublicKey>,
    state: NewcomerState,
}

impl Newcomer {
    /// Prepares a newcomer with fresh device records for `display_name` on
    /// `platform`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Randomness`] if the one-time code cannot be drawn.
    pub fn new(
        binding: SigningKey,
        principal: SigningKey,
        display_name: &str,
        platform: &str,
        now: Timestamp,
    ) -> Result<Self, Error> {
        let mut raw = [0u8; 8];
        getrandom::fill(&mut raw).map_err(|e| Error::Randomness(e.to_string()))?;
        let code: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let principal_fact = DeviceFact {
            key: *principal.public(),
            kind: DeviceKind::Principal,
            display_name: display_name.to_owned(),
            platform: platform.to_owned(),
            registration: Some(Registration::Personal),
            person: None,
            principal: None,
            introducer: false,
            created_at: now,
        };
        let binding_fact = DeviceFact {
            key: *binding.public(),
            kind: DeviceKind::Binding,
            display_name: display_name.to_owned(),
            platform: platform.to_owned(),
            registration: None,
            person: None,
            principal: Some(*principal.public()),
            introducer: false,
            created_at: now,
        };
        let devices = vec![
            facts::device_fact(&binding, &binding_fact)?,
            facts::device_fact(&principal, &principal_fact)?,
        ];
        Ok(Self {
            binding,
            principal,
            code,
            devices,
            introducer: None,
            state: NewcomerState::Offering,
        })
    }

    /// The offer to show, dialable at `addrs`.
    #[must_use]
    pub fn offer(&self, addrs: Vec<SocketAddr>) -> Offer {
        Offer {
            binding: *self.binding.public(),
            principal: *self.principal.public(),
            addrs,
            code: self.code.clone(),
        }
    }

    /// Where the ceremony is.
    #[must_use]
    pub const fn state(&self) -> NewcomerState {
        self.state
    }

    /// The binding key: the key the pairing connection authenticates.
    #[must_use]
    pub const fn binding(&self) -> &SigningKey {
        &self.binding
    }

    /// The principal key.
    #[must_use]
    pub const fn principal(&self) -> &SigningKey {
        &self.principal
    }

    /// An introducer connected (its authenticated key is `introducer`) and
    /// said hello. Returns the `ready` reply if the one-time code matches.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] on a wrong code or out of order.
    pub fn on_hello(&mut self, introducer: PublicKey, hello: &Hello) -> Result<Ready, Error> {
        if self.state != NewcomerState::Offering {
            return Err(self.abort("hello out of order"));
        }
        if hello.code != self.code {
            return Err(self.abort("one-time code does not match the offer"));
        }
        self.introducer = Some(introducer);
        self.state = NewcomerState::Greeted;
        Ok(Ready {
            message_type: TypeName::new(types::READY),
            devices: self.devices.clone(),
        })
    }

    /// The exporter context for this ceremony.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] before the hello.
    pub fn exporter_context(&self) -> Result<[u8; 32], Error> {
        let introducer = self
            .introducer
            .ok_or_else(|| Error::Chain("no introducer yet".to_owned()))?;
        Ok(exporter_context(self.binding.public(), &introducer))
    }

    /// The user confirmed the codes match. Returns the message to send.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] out of order.
    pub fn confirm(&mut self) -> Result<Confirmed, Error> {
        if self.state != NewcomerState::Greeted {
            return Err(self.abort("confirm out of order"));
        }
        self.state = NewcomerState::Confirmed;
        Ok(Confirmed {
            message_type: TypeName::new(types::CONFIRMED),
        })
    }

    /// The grant arrived. Accepted only after this side confirmed; the person
    /// fact must verify and both roles must chain to it over my keys.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] if anything is out of order or does not chain.
    pub fn on_grant(&mut self, grant: Grant) -> Result<(Joined, Done), Error> {
        if self.state != NewcomerState::Confirmed {
            return Err(self.abort("grant before confirmation"));
        }
        let person = facts::verify_person(&grant.person)
            .map_err(|e| self.abort(&format!("person fact: {e}")))?;
        let mut saw_binding = false;
        let mut saw_principal = false;
        for role in &grant.roles {
            let fact =
                facts::verify_role(role, &person).map_err(|e| self.abort(&format!("role: {e}")))?;
            match fact.role {
                Role::Binding if fact.subject == *self.binding.public() => saw_binding = true,
                Role::Principal if fact.subject == *self.principal.public() => saw_principal = true,
                _ => return Err(self.abort("role over a key that is not mine")),
            }
        }
        if !saw_binding || !saw_principal {
            return Err(self.abort("grant lacks a binding or principal role"));
        }
        let self_signing = match grant.self_signing {
            Some(seed) => {
                let seed: [u8; 32] = seed
                    .0
                    .as_slice()
                    .try_into()
                    .map_err(|_| self.abort("self-signing seed is not 32 bytes"))?;
                let key = SigningKey::from_seed(seed);
                if *key.public() != person.self_signing_key.key {
                    return Err(self.abort("self-signing seed does not match the person fact"));
                }
                Some(key)
            }
            None => None,
        };
        self.state = NewcomerState::Done;
        Ok((
            Joined {
                person: grant.person,
                roles: grant.roles,
                devices: self.devices.clone(),
                self_signing,
                gossip: grant.gossip,
            },
            Done {
                message_type: TypeName::new(types::DONE),
            },
        ))
    }

    /// Stops the ceremony.
    pub fn abort(&mut self, reason: &str) -> Error {
        self.state = NewcomerState::Aborted;
        Error::Chain(format!("pairing aborted: {reason}"))
    }
}

/// Where the introducer is in the ceremony.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntroducerState {
    /// Connected; hello not yet answered.
    Greeting,
    /// Ready received; codes can be derived and shown.
    Greeted,
    /// Both sides confirmed; grant sent, waiting for done.
    Granted,
    /// Done.
    Done,
    /// Stopped.
    Aborted,
}

/// What an introducer ends the ceremony with, for the daemon to persist.
#[derive(Debug, Clone)]
pub struct Introduced {
    /// The newcomer's device records.
    pub devices: Vec<Envelope>,
    /// The roles I signed over its keys.
    pub roles: Vec<Envelope>,
}

/// The introducer's side.
#[derive(Debug)]
pub struct Introducer {
    keys: PersonKeys,
    person: Envelope,
    my_tls_key: PublicKey,
    offer: Offer,
    make_introducer: bool,
    gossip: Vec<Envelope>,
    devices: Vec<Envelope>,
    roles: Vec<Envelope>,
    i_confirmed: bool,
    peer_confirmed: bool,
    state: IntroducerState,
}

impl Introducer {
    /// Prepares an introducer: my person keys and current person fact, the
    /// key my pairing connection authenticates with, the scanned offer,
    /// whether the newcomer becomes an introducer, and the gossip to hand over.
    #[must_use]
    pub fn new(
        keys: PersonKeys,
        person: Envelope,
        my_tls_key: PublicKey,
        offer: Offer,
        make_introducer: bool,
        gossip: Vec<Envelope>,
    ) -> Self {
        Self {
            keys,
            person,
            my_tls_key,
            offer,
            make_introducer,
            gossip,
            devices: Vec::new(),
            roles: Vec::new(),
            i_confirmed: false,
            peer_confirmed: false,
            state: IntroducerState::Greeting,
        }
    }

    /// The key the pairing connection must authenticate: the offer's binding
    /// key. A connection to any other key is not the device that was scanned.
    #[must_use]
    pub const fn expected_peer(&self) -> &PublicKey {
        &self.offer.binding
    }

    /// Where the ceremony is.
    #[must_use]
    pub const fn state(&self) -> IntroducerState {
        self.state
    }

    /// The first message: the offer's one-time code.
    #[must_use]
    pub fn hello(&self) -> Hello {
        Hello {
            message_type: TypeName::new(types::HELLO),
            code: self.offer.code.clone(),
        }
    }

    /// The newcomer answered with its device records: self-signed, over the
    /// two keys in the offer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] if a record does not verify or names another key.
    pub fn on_ready(&mut self, ready: Ready) -> Result<(), Error> {
        if self.state != IntroducerState::Greeting {
            return Err(self.abort("ready out of order"));
        }
        let mut keys = Vec::new();
        for env in &ready.devices {
            let fact = facts::verify_device(env)
                .map_err(|e| self.abort(&format!("device record: {e}")))?;
            if fact.key != self.offer.binding && fact.key != self.offer.principal {
                return Err(self.abort("device record for a key not in the offer"));
            }
            keys.push(fact.key);
        }
        if !keys.contains(&self.offer.binding) || !keys.contains(&self.offer.principal) {
            return Err(self.abort("device records do not cover both offered keys"));
        }
        self.devices = ready.devices;
        self.state = IntroducerState::Greeted;
        Ok(())
    }

    /// The exporter context for this ceremony.
    #[must_use]
    pub fn exporter_context(&self) -> [u8; 32] {
        exporter_context(&self.offer.binding, &self.my_tls_key)
    }

    /// The user confirmed the codes match. Returns the message to send.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] out of order.
    pub fn confirm(&mut self) -> Result<Confirmed, Error> {
        if self.state != IntroducerState::Greeted {
            return Err(self.abort("confirm out of order"));
        }
        self.i_confirmed = true;
        Ok(Confirmed {
            message_type: TypeName::new(types::CONFIRMED),
        })
    }

    /// The newcomer's user confirmed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] out of order.
    pub fn on_confirmed(&mut self) -> Result<(), Error> {
        if self.state != IntroducerState::Greeted {
            return Err(self.abort("peer confirmed out of order"));
        }
        self.peer_confirmed = true;
        Ok(())
    }

    /// The grant, available once both sides have confirmed. Signs the roles.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if encoding fails.
    pub fn grant(&mut self, now: Timestamp) -> Result<Option<Grant>, Error> {
        if self.state != IntroducerState::Greeted || !(self.i_confirmed && self.peer_confirmed) {
            return Ok(None);
        }
        let roles = grant_roles(&self.keys, &self.offer, self.make_introducer, now)?;
        self.roles = roles.clone();
        self.state = IntroducerState::Granted;
        Ok(Some(Grant {
            message_type: TypeName::new(types::GRANT),
            person: self.person.clone(),
            roles,
            self_signing: self
                .make_introducer
                .then(|| Bytes(self.keys.self_signing.seed().to_vec())),
            gossip: self.gossip.clone(),
        }))
    }

    /// The newcomer stored everything.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] out of order.
    pub fn on_done(&mut self) -> Result<Introduced, Error> {
        if self.state != IntroducerState::Granted {
            return Err(self.abort("done out of order"));
        }
        self.state = IntroducerState::Done;
        Ok(Introduced {
            devices: self.devices.clone(),
            roles: self.roles.clone(),
        })
    }

    /// Stops the ceremony.
    pub fn abort(&mut self, reason: &str) -> Error {
        self.state = IntroducerState::Aborted;
        Error::Chain(format!("pairing aborted: {reason}"))
    }
}

/// Helper for an introducer to build the grant's role facts.
///
/// # Errors
///
/// Returns [`Error::Decode`] if encoding fails.
pub fn grant_roles(
    keys: &PersonKeys,
    offer: &Offer,
    make_introducer: bool,
    now: Timestamp,
) -> Result<Vec<Envelope>, Error> {
    Ok(vec![
        facts::role_fact(keys, Role::Binding, offer.binding, make_introducer, now)?,
        facts::role_fact(keys, Role::Principal, offer.principal, false, now)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        Timestamp::from_unix(1_760_000_000).unwrap()
    }

    #[test]
    fn the_two_state_machines_complete_a_ceremony() {
        let alice = PersonKeys::generate().unwrap();
        let person = facts::person_fact(&alice, "Alice", 1, now()).unwrap();
        let phone = SigningKey::generate().unwrap(); // Alice's existing binding
        let mut n = newcomer();
        let offer = n.offer(vec!["127.0.0.1:1".parse().unwrap()]);
        let mut i = Introducer::new(
            alice.clone(),
            person,
            *phone.public(),
            offer.clone(),
            true,
            vec![],
        );
        assert_eq!(i.expected_peer(), n.binding().public());

        let ready = n.on_hello(*phone.public(), &i.hello()).unwrap();
        i.on_ready(ready).unwrap();
        assert_eq!(n.exporter_context().unwrap(), i.exporter_context());
        let exporter = [7u8; 32];
        assert_eq!(
            short_code(&exporter).unwrap(),
            short_code(&exporter).unwrap()
        );

        // Newcomer confirms first; the introducer has no grant yet.
        let c = n.confirm().unwrap();
        assert_eq!(c.message_type.as_str(), types::CONFIRMED);
        i.on_confirmed().unwrap();
        assert!(
            i.grant(now()).unwrap().is_none(),
            "needs my confirmation too"
        );
        i.confirm().unwrap();
        let grant = i.grant(now()).unwrap().expect("both confirmed");
        assert!(grant.self_signing.is_some());
        let (joined, done) = n.on_grant(grant).unwrap();
        assert_eq!(joined.devices.len(), 2);
        let _ = done;
        let introduced = i.on_done().unwrap();
        assert_eq!(introduced.roles.len(), 2);
        assert_eq!(introduced.devices.len(), 2);
        assert_eq!(i.state(), IntroducerState::Done);
    }

    #[test]
    fn the_introducer_rejects_records_for_other_keys() {
        let alice = PersonKeys::generate().unwrap();
        let person = facts::person_fact(&alice, "Alice", 1, now()).unwrap();
        let n = newcomer();
        let other = newcomer();
        let offer = n.offer(vec!["127.0.0.1:1".parse().unwrap()]);
        let mut i = Introducer::new(
            alice,
            person,
            PublicKey::from_bytes([3; 32]),
            offer,
            false,
            vec![],
        );
        let mut other = other;
        let other_offer = other.offer(vec!["127.0.0.1:1".parse().unwrap()]);
        let ready = other
            .on_hello(PublicKey::from_bytes([3; 32]), &hello(&other_offer.code))
            .unwrap();
        assert!(i.on_ready(ready).is_err());
        assert_eq!(i.state(), IntroducerState::Aborted);
    }

    fn newcomer() -> Newcomer {
        Newcomer::new(
            SigningKey::generate().unwrap(),
            SigningKey::generate().unwrap(),
            "laptop",
            "linux",
            now(),
        )
        .unwrap()
    }

    fn hello(code: &str) -> Hello {
        Hello {
            message_type: TypeName::new(types::HELLO),
            code: code.to_owned(),
        }
    }

    #[test]
    fn offers_round_trip_as_text() {
        let n = newcomer();
        let offer = n.offer(vec!["192.168.1.10:40123".parse().unwrap()]);
        let text = offer.to_text();
        assert!(text.starts_with("ligature:pair/1?b=z6Mk"));
        assert_eq!(Offer::parse(&text).unwrap(), offer);
        assert!(Offer::parse("ligature:pair/1?b=zz&c=0123456789abcdef").is_err());
        assert!(Offer::parse("http://example").is_err());
    }

    #[test]
    fn the_short_code_is_six_digits_from_the_first_twenty_bits() {
        assert_eq!(short_code(&[0, 0, 0, 0, 9, 9]).unwrap(), "000000");
        assert_eq!(
            short_code(&[0xff; 32]).unwrap(),
            format!("{:06}", 0xfffff % 1_000_000)
        );
        assert_eq!(short_code(&[0x12, 0x34, 0x56, 0x78]).unwrap(), "074565");
        assert!(short_code(&[1, 2, 3]).is_err());
        let ctx = exporter_context(
            &PublicKey::from_bytes([1; 32]),
            &PublicKey::from_bytes([2; 32]),
        );
        assert_ne!(
            ctx,
            exporter_context(
                &PublicKey::from_bytes([2; 32]),
                &PublicKey::from_bytes([1; 32])
            )
        );
    }

    #[test]
    fn a_wrong_one_time_code_aborts() {
        let mut n = newcomer();
        let introducer = SigningKey::generate().unwrap();
        assert!(
            n.on_hello(*introducer.public(), &hello("0000000000000000"))
                .is_err()
        );
        assert_eq!(n.state(), NewcomerState::Aborted);
    }

    #[test]
    fn the_newcomer_refuses_a_grant_before_confirming() {
        let mut n = newcomer();
        let alice = PersonKeys::generate().unwrap();
        let offer = n.offer(vec!["127.0.0.1:1".parse().unwrap()]);
        let ready = n
            .on_hello(*alice.self_signing.public(), &hello(&offer.code))
            .unwrap();
        assert_eq!(ready.devices.len(), 2);
        for d in &ready.devices {
            facts::verify_device(d).unwrap();
        }
        let grant = Grant {
            message_type: TypeName::new(types::GRANT),
            person: facts::person_fact(&alice, "Alice", 1, now()).unwrap(),
            roles: grant_roles(&alice, &offer, true, now()).unwrap(),
            self_signing: Some(Bytes(alice.self_signing.seed().to_vec())),
            gossip: vec![],
        };
        assert!(n.on_grant(grant).is_err(), "before confirmation");
        assert_eq!(n.state(), NewcomerState::Aborted);
    }

    #[test]
    fn a_confirmed_newcomer_accepts_a_valid_grant_and_rejects_bad_ones() {
        let alice = PersonKeys::generate().unwrap();
        let person = facts::person_fact(&alice, "Alice", 1, now()).unwrap();
        let build = |roles: Vec<Envelope>, seed: Option<Bytes>| Grant {
            message_type: TypeName::new(types::GRANT),
            person: person.clone(),
            roles,
            self_signing: seed,
            gossip: vec![],
        };

        let mut n = newcomer();
        let offer = n.offer(vec!["127.0.0.1:1".parse().unwrap()]);
        n.on_hello(*alice.self_signing.public(), &hello(&offer.code))
            .unwrap();
        n.confirm().unwrap();
        let roles = grant_roles(&alice, &offer, true, now()).unwrap();
        let (joined, _done) = n
            .on_grant(build(
                roles.clone(),
                Some(Bytes(alice.self_signing.seed().to_vec())),
            ))
            .unwrap();
        assert_eq!(
            joined.self_signing.unwrap().public(),
            alice.self_signing.public()
        );
        assert_eq!(joined.roles.len(), 2);
        assert_eq!(n.state(), NewcomerState::Done);

        // Roles over someone else's keys.
        let mut n2 = newcomer();
        let offer2 = n2.offer(vec!["127.0.0.1:1".parse().unwrap()]);
        n2.on_hello(*alice.self_signing.public(), &hello(&offer2.code))
            .unwrap();
        n2.confirm().unwrap();
        assert!(n2.on_grant(build(roles.clone(), None)).is_err());

        // A seed that does not match the person fact.
        let mut n3 = newcomer();
        let offer3 = n3.offer(vec!["127.0.0.1:1".parse().unwrap()]);
        n3.on_hello(*alice.self_signing.public(), &hello(&offer3.code))
            .unwrap();
        n3.confirm().unwrap();
        let roles3 = grant_roles(&alice, &offer3, true, now()).unwrap();
        let wrong = Bytes(SigningKey::generate().unwrap().seed().to_vec());
        assert!(n3.on_grant(build(roles3, Some(wrong))).is_err());

        // Roles signed by an outsider.
        let mut n4 = newcomer();
        let offer4 = n4.offer(vec!["127.0.0.1:1".parse().unwrap()]);
        n4.on_hello(*alice.self_signing.public(), &hello(&offer4.code))
            .unwrap();
        n4.confirm().unwrap();
        let mallory = PersonKeys::generate().unwrap();
        let forged = grant_roles(&mallory, &offer4, false, now()).unwrap();
        assert!(n4.on_grant(build(forged, None)).is_err());
    }
}
