//! Optional key publication. Spec chapter 01 section 4.
//!
//! A person may publish their master key at a DNS name, as a TXT record or at
//! a well-known HTTPS path, to gain a human-readable name and a place to list
//! relays. Publication never establishes trust: knowing a person requires the
//! in-person ceremony (ADR 0030). This module will look such a key up and
//! verify that what is published matches. Nothing else in the workspace
//! depends on it.

/// Looks up and verifies an optionally published master key.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct Publication;
