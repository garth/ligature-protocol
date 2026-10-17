//! BLE discovery and wake. Spec chapter 04 section 4.2.
//!
//! Lets phones and low-power devices be found and woken, then bootstraps a QUIC
//! connection over IP. Also the path to iPhone accessory profiles (spec
//! chapter 08).

/// BLE advertiser and scanner.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct Ble;
