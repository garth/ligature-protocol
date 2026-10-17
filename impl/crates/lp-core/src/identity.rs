//! Identity identifiers. Spec chapter 01.

/// A person's master public key, as a multibase string with a multicodec
/// prefix. This is the root of trust for everything the person owns or knows.
/// Key handling lives in `lp-identity` and `lp-crypto`; this type is only the
/// identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PersonKey(pub String);

/// In-memory view of an `org.ligatureprotocol.person` fact: the master key,
/// the self-chosen display name, and (once implemented) the subordinate keys
/// with the master key's signatures over them. Spec chapter 01 section 4.
#[derive(Debug, Clone)]
pub struct PersonRecord {
    /// The person.
    pub key: PersonKey,
    /// Self-chosen; what a prompt shows instead of the key.
    pub display_name: String,
}

/// A group's public key, as a multibase string with a multicodec prefix. The
/// group's identity: it signs the membership document and owns group-owned
/// devices. Spec chapter 10.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupKey(pub String);
