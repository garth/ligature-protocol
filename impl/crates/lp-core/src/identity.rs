//! Identity identifiers. Spec chapter 01.

/// A person's master public key, as a multibase string with a multicodec
/// prefix. This is the root of trust for everything the person owns or knows.
/// Key handling lives in `lp-identity` and `lp-crypto`; this type is only the
/// identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PersonKey(pub String);

/// A DNS name at which a person has optionally published their master key,
/// such as `alice.example.org`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PublishedName(pub String);

/// Identifier of a group of people such as a household. Spec chapter 10.
///
/// Whether this is a key of its own or derived from the founding admin's key
/// is an open question; the newtype keeps callers independent of the answer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupId(pub String);
