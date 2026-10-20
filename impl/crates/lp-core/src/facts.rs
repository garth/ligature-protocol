//! The payloads of signed facts: person, role, device and revocation. Spec
//! chapters 01, 02 and 07 and the schemas of the same names under `schemas/`.
//!
//! These are the decoded views. On the wire each travels as the payload of an
//! [`crate::envelope::Envelope`], byte-exact. Decoding is forgiving: unknown
//! fields are ignored and absent optional fields take their defaults.

use serde::{Deserialize, Serialize};

use crate::PublicKey;
use crate::wire::{Bytes, Timestamp, TypeName};

/// Type name of a person fact.
pub const PERSON_TYPE: &str = "org.ligatureprotocol.person";
/// Type name of a role fact.
pub const ROLE_TYPE: &str = "org.ligatureprotocol.role";
/// Type name of a device record.
pub const DEVICE_TYPE: &str = "org.ligatureprotocol.device";
/// Type name of a revocation or abandonment.
pub const REVOCATION_TYPE: &str = "org.ligatureprotocol.revocation";
/// Type name of a grant.
pub const GRANT_TYPE: &str = "org.ligatureprotocol.grant";

/// The role a ceremony signature asserts about a key. Spec chapter 02 section 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// My account on a device: member of my device set.
    Binding,
    /// A device I use: I may request its device-scoped capabilities.
    Principal,
    /// Another person's master key that I have verified in person.
    Person,
}

/// A key endorsed by the master key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubordinateKey {
    /// The subordinate public key.
    pub key: PublicKey,
    /// Master signature over `"ligature/subkey/1" || purpose || key bytes`.
    pub master_signature: Bytes,
}

/// `org.ligatureprotocol.person`: the master key, the self-asserted display
/// name and both subordinate keys. Signed by the self-signing key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonFact {
    /// The person.
    pub master_key: PublicKey,
    /// Self-chosen; what a prompt shows instead of the key.
    pub display_name: String,
    /// Held by introducers; signs bindings and principals.
    pub self_signing_key: SubordinateKey,
    /// Signs known persons and group acceptances.
    pub user_signing_key: SubordinateKey,
    /// Increases with every change; the highest verified version wins.
    pub version: u64,
    /// When this version was made.
    pub updated_at: Timestamp,
}

/// `org.ligatureprotocol.role`: one role over one key. Signed by the
/// self-signing key (`binding`, `principal`) or the user-signing key (`person`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleFact {
    /// The key being signed.
    pub subject: PublicKey,
    /// What the signature asserts.
    pub role: Role,
    /// The master key on whose behalf the signature is made.
    pub person: PublicKey,
    /// For `binding`: whether the new binding holds the self-signing key.
    #[serde(default, skip_serializing_if = "is_false")]
    pub introducer: bool,
    /// When the ceremony happened.
    pub signed_at: Timestamp,
}

/// Principal or binding, as the device record states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    /// The device itself.
    Principal,
    /// One person's account on it.
    Binding,
}

/// Personal or group registration of a principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Registration {
    /// Owned by one or more people.
    Personal,
    /// Owned by the group.
    Group,
}

/// `org.ligatureprotocol.device`: a device's self-description, signed by the
/// device's own key. Roles over the key are separate facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFact {
    /// The principal or binding key: the identifier.
    pub key: PublicKey,
    /// Principal or binding.
    pub kind: DeviceKind,
    /// Host name by default; the user may override it.
    pub display_name: String,
    /// `linux`, `macos`, `android`, `ios`, `hub` or `other`.
    pub platform: String,
    /// For a principal: personal or group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration: Option<Registration>,
    /// For a binding: the account holder's master key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<PublicKey>,
    /// For a binding: the principal key of the device it runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<PublicKey>,
    /// For a binding: whether it holds the self-signing key.
    #[serde(default, skip_serializing_if = "is_false")]
    pub introducer: bool,
    /// When the record was made.
    pub created_at: Timestamp,
}

/// Whether a revocation drops a key from the set or abandons a master key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RevocationKind {
    /// A binding, principal or known person is no longer trusted.
    #[default]
    Revocation,
    /// A master key is abandoned after a rebuild (chapter 07).
    Abandonment,
}

/// `org.ligatureprotocol.revocation`. Signed by an introducer's self-signing key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevocationFact {
    /// The key no longer trusted.
    pub subject: PublicKey,
    /// Revocation or abandonment.
    #[serde(default)]
    pub kind: RevocationKind,
    /// The master key on whose behalf the revocation is made.
    pub person: PublicKey,
    /// When.
    pub revoked_at: Timestamp,
}

/// How a grant came about. Spec chapter 10 section 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantBasis {
    /// Same person, user-scoped capability: no prompt was needed.
    Implicit,
    /// A person approved it, once or as a standing grant.
    Approval,
    /// Device policy allowed it while nobody was present.
    Policy,
}

/// `org.ligatureprotocol.grant`: one device may use a capability of another.
/// Signed by the grantor's key; held by the device that issued it and never
/// gossiped (spec chapter 05 section 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantFact {
    /// The principal or binding offering the capability.
    pub grantor: PublicKey,
    /// The device permitted to use it.
    pub grantee: PublicKey,
    /// The grantee's owner, when it is another person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grantee_person: Option<PublicKey>,
    /// The capability type.
    pub capability: TypeName,
    /// How the grant came about.
    #[serde(default = "implicit")]
    pub basis: GrantBasis,
    /// Whether the approver chose to remember the answer.
    #[serde(default, skip_serializing_if = "is_false")]
    pub standing: bool,
    /// When the grant ends, if ever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// When the grant was withdrawn, if it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<Timestamp>,
    /// When the grant was made.
    pub created_at: Timestamp,
}

const fn implicit() -> GrantBasis {
    GrantBasis::Implicit
}

/// Which type name a fact payload type carries.
pub trait FactType {
    /// The reverse-DNS type name.
    const TYPE: &'static str;

    /// The type name as a [`TypeName`].
    #[must_use]
    fn type_name() -> TypeName {
        TypeName::new(Self::TYPE)
    }
}

impl FactType for PersonFact {
    const TYPE: &'static str = PERSON_TYPE;
}

impl FactType for RoleFact {
    const TYPE: &'static str = ROLE_TYPE;
}

impl FactType for RevocationFact {
    const TYPE: &'static str = REVOCATION_TYPE;
}

impl FactType for DeviceFact {
    const TYPE: &'static str = DEVICE_TYPE;
}

impl FactType for GrantFact {
    const TYPE: &'static str = GRANT_TYPE;
}

/// Fact types that stay on the device that made them and are never gossiped.
pub const LOCAL_ONLY_TYPES: [&str; 1] = [GRANT_TYPE];

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_fields_are_ignored_and_defaults_apply() {
        let json = r#"{"subject":"z6MkjDFtUBRdWTZuJW5Dzc2gnbAt3vXFp38AVYMDhjaACtkd",
            "role":"binding","person":"z6MkjDFtUBRdWTZuJW5Dzc2gnbAt3vXFp38AVYMDhjaACtkd",
            "signedAt":"2026-01-01T00:00:00Z","futureField":[1,2,3]}"#;
        let fact: RoleFact = serde_json::from_str(json).unwrap();
        assert!(!fact.introducer);
        assert_eq!(fact.role, Role::Binding);
        let rev: RevocationFact = serde_json::from_str(
            r#"{"subject":"z6MkjDFtUBRdWTZuJW5Dzc2gnbAt3vXFp38AVYMDhjaACtkd",
            "person":"z6MkjDFtUBRdWTZuJW5Dzc2gnbAt3vXFp38AVYMDhjaACtkd","revokedAt":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(rev.kind, RevocationKind::Revocation);
    }
}
