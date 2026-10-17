//! Signing keys. Spec chapter 01.
//!
//! Generates and stores master, self-signing, user-signing, principal and
//! binding keys in whichever configuration the user requires, and signs and
//! verifies role signatures, revocations and other gossiped facts. Public keys
//! are rendered as multibase strings with a multicodec prefix.

/// A signing key pair held by this device.
///
/// Not yet implemented.
#[derive(Debug)]
pub struct SigningKey;
