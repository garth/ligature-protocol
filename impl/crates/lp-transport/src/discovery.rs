//! Remote discovery and hole-punch coordination. Spec chapter 04 sections 5.1
//! and 5.2.
//!
//! Answers "where is device X now?" without trusting whoever answers:
//!
//! - Signed address records keyed by the device public key, published to a DHT
//!   (BitTorrent Mainline via pkarr, as iroh does).
//! - Relays, and optionally addresses, listed in the user's DID document.
//!
//! Records are signed by the device key; the connection itself re-authenticates
//! the peer, so a spoofed or poisoned record can at worst cause a failed dial.
//! The hole-punch coordinator exchanges observed addresses and learns nothing
//! else.

/// A signed record of a device's current reachable addresses and relays.
///
/// Not yet implemented.
#[derive(Debug)]
pub struct AddressRecord;

/// Publishes and looks up [`AddressRecord`]s through one or more sources.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct Discovery;

/// Coordinates a hole punch between two NATed peers.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct HolePunch;
