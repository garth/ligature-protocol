//! Gossip: what to tell a peer and what to accept. Spec chapter 06 section 4;
//! ADR 0036.
//!
//! After authentication each side lists the identifiers of the facts it holds
//! (`gossip.have`), asks for the ones it lacks (`gossip.want`) and sends each
//! requested fact (`gossip.fact`), revocations and abandonments first, then the
//! trust chain (person facts, roles), then device records, then everything
//! else. The receiver stores a fact only if its signature verifies over the
//! bytes as received; the chain is judged later, at recognition.

use std::collections::HashSet;

use lp_core::Error;
use lp_core::envelope::{Envelope, FactId};
use lp_core::facts::{DEVICE_TYPE, PERSON_TYPE, REVOCATION_TYPE, ROLE_TYPE};
use lp_core::frame::{GossipFact, GossipHave, GossipWant};

use crate::trust_store::{StoredFact, TrustStore};

/// The identifiers this device holds, as a `have` message.
///
/// # Errors
///
/// Returns a store error.
pub fn have(store: &TrustStore) -> Result<GossipHave, Error> {
    Ok(GossipHave::new(store.ids()?))
}

/// The identifiers the peer holds and we lack, as a `want` message.
///
/// # Errors
///
/// Returns a store error.
pub fn want(store: &TrustStore, theirs: &GossipHave) -> Result<GossipWant, Error> {
    let mine: HashSet<FactId> = store.ids()?.into_iter().collect();
    let missing = theirs
        .facts
        .iter()
        .filter(|id| !mine.contains(id))
        .copied()
        .collect();
    Ok(GossipWant::new(missing))
}

/// The rank a fact type is sent in: lower first.
fn rank(fact_type: &str) -> u8 {
    match fact_type {
        REVOCATION_TYPE => 0,
        PERSON_TYPE => 1,
        ROLE_TYPE => 2,
        DEVICE_TYPE => 3,
        _ => 4,
    }
}

/// The facts the peer asked for that we hold, in sending order: revocations
/// first, then person facts, roles, device records, the rest; oldest first
/// within a rank. Unknown identifiers are ignored.
///
/// # Errors
///
/// Returns a store error.
pub fn facts_for(store: &TrustStore, wanted: &GossipWant) -> Result<Vec<GossipFact>, Error> {
    let asked: HashSet<FactId> = wanted.facts.iter().copied().collect();
    let mut facts: Vec<StoredFact> = store
        .all()?
        .into_iter()
        .filter(|f| asked.contains(&f.id))
        .filter(|f| !lp_core::facts::LOCAL_ONLY_TYPES.contains(&f.envelope.fact_type.as_str()))
        .collect();
    facts.sort_by(|a, b| {
        rank(a.envelope.fact_type.as_str())
            .cmp(&rank(b.envelope.fact_type.as_str()))
            .then(a.created_at.cmp(&b.created_at))
            .then(a.id.cmp(&b.id))
    });
    Ok(facts
        .into_iter()
        .map(|f| GossipFact::new(f.envelope))
        .collect())
}

/// Stores a received fact if it verifies. Returns whether it was new, which
/// a copy that heals a quarantined row counts as. A fact of a local-only
/// type (a grant) is refused whoever sends it: it never leaves the device
/// that made it.
///
/// # Errors
///
/// Returns [`Error::InvalidSignature`] or [`Error::Decode`] for a bad fact
/// or a local-only one, which the caller discards and logs, or a store
/// error.
pub fn apply(store: &TrustStore, envelope: &Envelope) -> Result<bool, Error> {
    if lp_core::facts::LOCAL_ONLY_TYPES.contains(&envelope.fact_type.as_str()) {
        return Err(Error::Decode(format!(
            "{} is local to the device that made it and is not accepted from a peer",
            envelope.fact_type
        )));
    }
    let id = crate::facts::id(envelope);
    if store.usable(&id)? {
        return Ok(false);
    }
    store.insert(envelope)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cross_signing::PersonKeys;
    use crate::facts;
    use lp_core::facts::{RevocationKind, Role};
    use lp_core::wire::Timestamp;
    use lp_crypto::signing::SigningKey;

    fn at(offset: i64) -> Timestamp {
        Timestamp::from_unix(1_760_000_000 + offset).unwrap()
    }

    #[test]
    fn want_is_what_they_have_and_we_lack() {
        let a = TrustStore::in_memory().unwrap();
        let b = TrustStore::in_memory().unwrap();
        let alice = PersonKeys::generate().unwrap();
        let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
        let laptop = SigningKey::generate().unwrap();
        let role = facts::role_fact(&alice, Role::Binding, *laptop.public(), true, at(1)).unwrap();
        a.insert(&person).unwrap();
        a.insert(&role).unwrap();
        b.insert(&person).unwrap();
        let w = want(&b, &have(&a).unwrap()).unwrap();
        assert_eq!(w.facts, vec![facts::id(&role)]);
        assert!(want(&a, &have(&b).unwrap()).unwrap().facts.is_empty());
    }

    #[test]
    fn facts_go_out_revocations_first_then_the_chain() {
        let a = TrustStore::in_memory().unwrap();
        let alice = PersonKeys::generate().unwrap();
        let laptop = SigningKey::generate().unwrap();
        let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
        let role = facts::role_fact(&alice, Role::Binding, *laptop.public(), true, at(1)).unwrap();
        let rev =
            facts::revocation_fact(&alice, *laptop.public(), RevocationKind::Revocation, at(9))
                .unwrap();
        for e in [&role, &person, &rev] {
            a.insert(e).unwrap();
        }
        let all = GossipWant::new(a.ids().unwrap());
        let out = facts_for(&a, &all).unwrap();
        let types: Vec<&str> = out.iter().map(|f| f.envelope.fact_type.as_str()).collect();
        assert_eq!(types, vec![REVOCATION_TYPE, PERSON_TYPE, ROLE_TYPE]);
    }

    #[test]
    fn a_bad_signature_is_refused_and_not_stored() {
        let b = TrustStore::in_memory().unwrap();
        let alice = PersonKeys::generate().unwrap();
        let mut person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
        person.payload.0[7] ^= 1;
        assert!(apply(&b, &person).is_err());
        assert!(b.all().unwrap().is_empty());
        let good = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
        assert!(apply(&b, &good).unwrap());
        assert!(!apply(&b, &good).unwrap(), "already held");
    }
}
