//! The proximity ceremony. Spec chapter 02 sections 4 to 6.
//!
//! The newcomer shows a fingerprint and one-time code (on screen, or on the
//! pairing web page a headless device serves on the LAN); an introducer
//! verifies it, the user confirms the short code matches, and the introducer
//! signs the key with a role and hands over the self-signing key if the
//! newcomer is to be an introducer, plus current gossip. BLE, NFC or UWB may
//! strengthen the proximity check but are never required.

/// The newcomer's side of the ceremony.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct Newcomer;

/// The introducer's side of the ceremony.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct Introducer;

/// The pairing page a headless device serves until it is claimed.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct PairingPage;
