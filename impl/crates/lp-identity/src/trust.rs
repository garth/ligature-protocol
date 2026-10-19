//! Recognition: what the trust store says about a key. Spec chapter 03
//! section 4 and chapter 04 section 3.2.
//!
//! After the handshake the transport knows the peer's key; this module says
//! whether that key is a binding of mine, a principal I use, a binding of a
//! person I know, or a stranger who is offered nothing but the pairing
//! ceremony. Every answer is derived from verified facts in the store at the
//! moment of asking; nothing is cached, so a revocation takes effect on the
//! next connection.

use std::collections::HashMap;

use lp_core::facts::{PERSON_TYPE, PersonFact, Role};
use lp_core::{Error, PublicKey};

use crate::facts;
use crate::trust_store::TrustStore;

/// What a peer key is to this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recognition {
    /// My account on another device: a binding signed by my self-signing key.
    MyBinding {
        /// Whether that binding holds the self-signing key.
        introducer: bool,
    },
    /// A device I use: a principal signed by my self-signing key.
    MyPrincipal,
    /// A binding of a person I have verified in person.
    KnownPersonBinding {
        /// Their master key.
        person: PublicKey,
        /// Their self-asserted name, from their person fact.
        display_name: String,
    },
    /// Nobody I know. Offered the pairing ceremony and nothing else.
    Stranger,
    /// Formerly trusted; a revocation names this key. Treated as a stranger
    /// and worth logging.
    Revoked,
}

impl Recognition {
    /// Whether the peer may proceed past the pairing offer.
    #[must_use]
    pub const fn is_trusted(&self) -> bool {
        matches!(
            self,
            Self::MyBinding { .. } | Self::MyPrincipal | Self::KnownPersonBinding { .. }
        )
    }
}

/// The verified person facts in the store, newest version per master key.
fn persons(store: &TrustStore) -> Result<HashMap<PublicKey, PersonFact>, Error> {
    let mut out: HashMap<PublicKey, PersonFact> = HashMap::new();
    for stored in store.of_type(PERSON_TYPE)? {
        let Ok(fact) = facts::verify_person(&stored.envelope) else {
            tracing::warn!(fact = %stored.id, "person fact in store does not verify; ignoring");
            continue;
        };
        if store.is_revoked(&fact.master_key)? {
            continue;
        }
        let replace = out
            .get(&fact.master_key)
            .is_none_or(|existing| fact.version > existing.version);
        if replace {
            out.insert(fact.master_key, fact);
        }
    }
    Ok(out)
}

/// Decides what `peer` is to the device whose person is `owner`.
///
/// # Errors
///
/// Returns [`Error::Store`] on a database error and [`Error::Chain`] if the
/// owner's own person fact is missing.
pub fn recognise(
    store: &TrustStore,
    owner: &PublicKey,
    peer: &PublicKey,
) -> Result<Recognition, Error> {
    if store.is_revoked(peer)? {
        return Ok(Recognition::Revoked);
    }
    let persons = persons(store)?;
    let me = persons
        .get(owner)
        .ok_or_else(|| Error::Chain("my own person fact is not in the store".to_owned()))?;

    // Roles I have signed over the peer.
    for stored in store.about(peer)? {
        if stored.envelope.fact_type.as_str() != lp_core::facts::ROLE_TYPE {
            continue;
        }
        if let Ok(role) = facts::verify_role(&stored.envelope, me) {
            match role.role {
                Role::Binding => {
                    return Ok(Recognition::MyBinding {
                        introducer: role.introducer,
                    });
                }
                Role::Principal => return Ok(Recognition::MyPrincipal),
                Role::Person => {}
            }
        }
    }

    // Bindings of people I know: a `person` role from me over their master
    // key, and a `binding` role from them over the peer.
    for (master, person) in &persons {
        if master == owner || store.is_revoked(master)? {
            continue;
        }
        let i_know_them = store
            .about(master)?
            .iter()
            .any(|s| facts::verify_role(&s.envelope, me).is_ok_and(|r| r.role == Role::Person));
        if !i_know_them {
            continue;
        }
        let their_binding = store.about(peer)?.iter().any(|s| {
            facts::verify_role(&s.envelope, person).is_ok_and(|r| r.role == Role::Binding)
        });
        if their_binding {
            return Ok(Recognition::KnownPersonBinding {
                person: *master,
                display_name: person.display_name.clone(),
            });
        }
    }
    Ok(Recognition::Stranger)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cross_signing::PersonKeys;
    use lp_core::facts::RevocationKind;
    use lp_core::wire::Timestamp;
    use lp_crypto::signing::SigningKey;

    fn at(offset: i64) -> Timestamp {
        Timestamp::from_unix(1_760_000_000 + offset).unwrap()
    }

    struct World {
        store: TrustStore,
        alice: PersonKeys,
        laptop: SigningKey,
    }

    fn world() -> World {
        let store = TrustStore::in_memory().unwrap();
        let alice = PersonKeys::generate().unwrap();
        store
            .insert(&facts::person_fact(&alice, "Alice", 1, at(0)).unwrap())
            .unwrap();
        let laptop = SigningKey::generate().unwrap();
        store
            .insert(
                &facts::role_fact(&alice, Role::Binding, *laptop.public(), true, at(1)).unwrap(),
            )
            .unwrap();
        World {
            store,
            alice,
            laptop,
        }
    }

    #[test]
    fn my_binding_is_recognised_with_its_introducer_flag() {
        let w = world();
        let r = recognise(&w.store, w.alice.person(), w.laptop.public()).unwrap();
        assert_eq!(r, Recognition::MyBinding { introducer: true });
        assert!(r.is_trusted());
    }

    #[test]
    fn my_principal_is_recognised() {
        let w = world();
        let tv = SigningKey::generate().unwrap();
        w.store
            .insert(
                &facts::role_fact(&w.alice, Role::Principal, *tv.public(), false, at(2)).unwrap(),
            )
            .unwrap();
        assert_eq!(
            recognise(&w.store, w.alice.person(), tv.public()).unwrap(),
            Recognition::MyPrincipal
        );
    }

    #[test]
    fn an_unknown_key_is_a_stranger() {
        let w = world();
        let stranger = SigningKey::generate().unwrap();
        let r = recognise(&w.store, w.alice.person(), stranger.public()).unwrap();
        assert_eq!(r, Recognition::Stranger);
        assert!(!r.is_trusted());
    }

    #[test]
    fn a_revoked_binding_is_refused() {
        let w = world();
        w.store
            .insert(
                &facts::revocation_fact(
                    &w.alice,
                    *w.laptop.public(),
                    RevocationKind::Revocation,
                    at(5),
                )
                .unwrap(),
            )
            .unwrap();
        let r = recognise(&w.store, w.alice.person(), w.laptop.public()).unwrap();
        assert_eq!(r, Recognition::Revoked);
        assert!(!r.is_trusted());
    }

    #[test]
    fn a_role_signed_by_someone_else_does_not_count() {
        let w = world();
        let mallory = PersonKeys::generate().unwrap();
        let phone = SigningKey::generate().unwrap();
        // Mallory's person fact and her claim that phone is Alice's binding.
        w.store
            .insert(&facts::person_fact(&mallory, "Mallory", 1, at(0)).unwrap())
            .unwrap();
        let forged = lp_core::facts::RoleFact {
            subject: *phone.public(),
            role: Role::Binding,
            person: *w.alice.person(),
            introducer: false,
            signed_at: at(3),
        };
        w.store
            .insert(&facts::sign(&mallory.self_signing, &forged).unwrap())
            .unwrap();
        assert_eq!(
            recognise(&w.store, w.alice.person(), phone.public()).unwrap(),
            Recognition::Stranger
        );
    }

    #[test]
    fn a_known_persons_binding_is_recognised_as_theirs() {
        let w = world();
        let bob = PersonKeys::generate().unwrap();
        let bobs_phone = SigningKey::generate().unwrap();
        w.store
            .insert(&facts::person_fact(&bob, "Bob", 1, at(0)).unwrap())
            .unwrap();
        w.store
            .insert(
                &facts::role_fact(&bob, Role::Binding, *bobs_phone.public(), true, at(1)).unwrap(),
            )
            .unwrap();
        // Before Alice knows Bob, his phone is a stranger.
        assert_eq!(
            recognise(&w.store, w.alice.person(), bobs_phone.public()).unwrap(),
            Recognition::Stranger
        );
        w.store
            .insert(&facts::role_fact(&w.alice, Role::Person, *bob.person(), false, at(2)).unwrap())
            .unwrap();
        assert_eq!(
            recognise(&w.store, w.alice.person(), bobs_phone.public()).unwrap(),
            Recognition::KnownPersonBinding {
                person: *bob.person(),
                display_name: "Bob".to_owned()
            }
        );
        // Revoking the person removes their devices too.
        w.store
            .insert(
                &facts::revocation_fact(&w.alice, *bob.person(), RevocationKind::Revocation, at(3))
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            recognise(&w.store, w.alice.person(), bobs_phone.public()).unwrap(),
            Recognition::Stranger
        );
    }

    #[test]
    fn without_my_own_person_fact_nothing_can_be_decided() {
        let store = TrustStore::in_memory().unwrap();
        let alice = PersonKeys::generate().unwrap();
        let k = SigningKey::generate().unwrap();
        assert!(matches!(
            recognise(&store, alice.person(), k.public()),
            Err(Error::Chain(_))
        ));
    }
}
