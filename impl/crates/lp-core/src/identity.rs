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
