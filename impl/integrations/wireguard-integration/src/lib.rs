//! **Non-core integration.** Ligature Protocol does not depend on WireGuard.
//!
//! This crate will generate Headscale/WireGuard peer configurations from the
//! user's LP device list, for users who want full network access to their home
//! (SSH, NAS, media) in addition to LP's capability model. See
//! [ADR 0013](https://github.com/ligature-protocol/ligature-protocol/blob/main/docs/adr/0013-wireguard-optional-integration.md).
//!
//! Why it is not core: WireGuard needs a virtual network interface, which takes
//! the single VPN slot on Android and iOS and needs elevated privileges on
//! desktops. LP's own transport is application-level QUIC (`lp-transport`).
//!
//! This crate depends on `lp-core` only. Nothing in the core crates or the
//! daemon may depend on it.
//!
//! **Status:** placeholder. No logic.

/// Produces WireGuard/Headscale peer configuration from LP device records.
///
/// Not yet implemented.
#[derive(Debug, Default)]
pub struct ConfigGenerator;
