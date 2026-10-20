//! Grants. Spec chapter 05 section 4; milestone 1 task E6.
//!
//! A binding of the same person using a user-scoped capability needs no
//! prompt, but the grant path exists end to end: the first use records a
//! grant with basis `implicit`, signed by this device's binding key and kept
//! in the local store. Grants are never gossiped. Anything across people is
//! refused in this milestone.

use std::sync::Arc;

use lp_core::capability::CLIPBOARD_TYPE;
use lp_core::facts::{GRANT_TYPE, GrantBasis, GrantFact};
use lp_core::wire::{Timestamp, TypeName};
use lp_core::{Error, PublicKey};
use lp_identity::facts;
use lp_identity::trust::Recognition;

use crate::daemon::Daemon;

/// One grant as `grants` lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantEntry {
    /// The device offering the capability.
    pub grantor: PublicKey,
    /// The device permitted to use it.
    pub grantee: PublicKey,
    /// The capability.
    pub capability: String,
    /// `implicit`, `approval` or `policy`.
    pub basis: GrantBasis,
    /// When it was made.
    pub created_at: Timestamp,
}

/// The `grants` result.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Grants {
    /// Grants this device issued.
    pub grants: Vec<GrantEntry>,
}

/// Every verified grant this device issued.
///
/// # Errors
///
/// Returns a store error.
pub fn list(daemon: &Daemon) -> Result<Grants, Error> {
    let me = *daemon.binding().public();
    let mut grants = Vec::new();
    for stored in daemon.store().of_type(GRANT_TYPE)? {
        if stored.envelope.signer != me {
            continue;
        }
        if facts::verify_signature(&stored.envelope).is_err() {
            continue;
        }
        let Ok(g) = facts::decode::<GrantFact>(&stored.envelope) else {
            continue;
        };
        if g.grantor != me || g.revoked_at.is_some() {
            continue;
        }
        grants.push(GrantEntry {
            grantor: g.grantor,
            grantee: g.grantee,
            capability: g.capability.to_string(),
            basis: g.basis,
            created_at: g.created_at,
        });
    }
    Ok(Grants { grants })
}

/// Whether a usable grant from this device to `grantee` for `capability` exists.
///
/// # Errors
///
/// Returns a store error.
pub fn exists(daemon: &Daemon, grantee: &PublicKey, capability: &str) -> Result<bool, Error> {
    Ok(list(daemon)?
        .grants
        .iter()
        .any(|g| g.grantee == *grantee && g.capability == capability))
}

/// A binding of the same person used `capability` offered by this device:
/// records an implicit grant unless one exists. Any other requester is
/// refused.
///
/// # Errors
///
/// Returns [`Error::Chain`] for a requester that is not a binding of this
/// person, or a store error.
pub fn grant_implicit(
    daemon: &Daemon,
    grantee: &PublicKey,
    capability: &str,
) -> Result<bool, Error> {
    match daemon.recognise(grantee)? {
        Recognition::MyBinding { .. } => {}
        other => {
            return Err(Error::Chain(format!(
                "no implicit grant for {grantee}: {other:?}; cross-person grants are not in this milestone"
            )));
        }
    }
    if exists(daemon, grantee, capability)? {
        return Ok(false);
    }
    let fact = GrantFact {
        grantor: *daemon.binding().public(),
        grantee: *grantee,
        grantee_person: None,
        capability: TypeName::new(capability),
        basis: GrantBasis::Implicit,
        standing: true,
        expires_at: None,
        revoked_at: None,
        created_at: Timestamp::now(),
    };
    let env = facts::sign(daemon.binding(), &fact)?;
    daemon.store().insert(&env)?;
    tracing::info!(%grantee, capability, "implicit grant recorded");
    Ok(true)
}

/// A binding of the same person set our clipboard: record the implicit grant,
/// logging rather than failing if the store refuses.
pub fn record_implicit(daemon: &Arc<Daemon>, peer: &PublicKey) {
    if let Err(e) = grant_implicit(daemon, peer, CLIPBOARD_TYPE) {
        tracing::warn!(%peer, %e, "could not record the implicit grant");
    }
}
