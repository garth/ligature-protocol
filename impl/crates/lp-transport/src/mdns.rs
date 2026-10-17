//! mDNS/DNS-SD local discovery. Spec chapter 04 section 4.1.
//!
//! Advertises an LP service instance and browses for peers on the LAN. The TXT
//! record carries a per-epoch identifier so devices in the same set recognise
//! each other without revealing the user's identity to bystanders. A found peer
//! is dialled directly with [`crate::quic`]; no helper is involved.

/// mDNS advertiser and browser.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct Mdns;
