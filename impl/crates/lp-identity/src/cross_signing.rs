//! Keys and role signatures. Spec chapters 01 and 02.
//!
//! Follows the Matrix model with a role attached: a master key signs a
//! self-signing key (used by introducers to sign bindings and principals) and
//! a user-signing key (used to sign known persons and group acceptances). In
//! the simplest configuration all three are one key. This module will create
//! keys in whichever configuration the user requires, produce role signatures,
//! and verify chains back to a master key.

/// The role a ceremony signature asserts about a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// My account on a device: member of my device set.
    Binding,
    /// A device I use: I may request its device-scoped capabilities.
    Principal,
    /// Another person's master key that I have verified in person.
    Person,
}

/// A person's key material in the configuration they chose.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct PersonKeys;

/// A role signature over a key, verifiable against a master key.
///
/// Not yet implemented.
#[derive(Debug)]
pub struct RoleSignature;
