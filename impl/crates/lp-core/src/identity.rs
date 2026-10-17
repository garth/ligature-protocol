//! User identity identifiers. Spec chapter 01.

/// A user's AT Protocol DID, such as `did:plc:...` or `did:web:...`.
///
/// This is the root of trust for a device set. Resolution and verification
/// live in `lp-identity`; this type is only the identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Did(pub String);

/// A DNS handle that resolves to a [`Did`], such as `alice.example.org`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Handle(pub String);

/// Identifier of a group of identities such as a household. Spec chapter 10.
///
/// Whether this is a DID of its own or a record key in an admin's Space is an
/// open question; the newtype keeps callers independent of the answer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupId(pub String);
