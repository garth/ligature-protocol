//! mDNS/DNS-SD local discovery. Spec chapter 04 section 2.
//!
//! Advertises an LP service instance and browses for peers. The TXT record
//! design must avoid revealing the user's identity to bystanders.

/// mDNS advertiser and browser.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct Mdns;
