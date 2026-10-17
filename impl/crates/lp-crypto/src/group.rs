//! The device-set MLS group.
//!
//! Planned responsibilities:
//!
//! - Create the group on the first device.
//! - Generate key packages for new devices and commit their addition.
//! - Remove revoked devices.
//! - Encrypt and decrypt application messages.
//! - Derive exporter secrets for bulk streams.

/// Handle to the device set's MLS group.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct DeviceGroup;
