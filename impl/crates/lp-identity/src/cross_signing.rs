//! Device cross-signing. Spec chapter 02.
//!
//! Follows the Matrix model: a master key signs a self-signing key, which
//! signs device keys. This module will create and verify those chains and
//! produce the signatures published in `org.ligatureprotocol.device` records.

/// A device's cross-signing state and operations.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct CrossSigning;
