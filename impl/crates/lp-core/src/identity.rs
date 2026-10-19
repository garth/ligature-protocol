//! Identity records. Spec chapter 01.
//!
//! Keys are identifiers ([`crate::key`]). This module holds the in-memory view
//! of the person fact.

pub use crate::key::{GroupKey, PersonKey};

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
