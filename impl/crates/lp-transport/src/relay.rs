//! Relay client. Spec chapter 04 section 5.3.
//!
//! When a direct QUIC path cannot be established (mobile and CGNAT networks),
//! traffic is forwarded through a relay. The relay sees QUIC packets or MLS
//! ciphertext and can read neither. Anyone can run one; the default personal
//! relay is the user's home hub; clients hold a list and route around failures.
//!
//! This module is the *client* side. A relay *server* is a separate program
//! that may be added to the workspace later.

/// A relay the client knows about, from the user's own records.
///
/// Not yet implemented.
#[derive(Debug)]
pub struct RelayEntry;

/// Selects among configured relays and opens relayed paths.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct RelayClient;
