//! Grants. Spec chapter 05 section 4; milestone 1 task E6.

use std::sync::Arc;

use lp_core::PublicKey;

use crate::daemon::Daemon;

/// A binding of the same person used the clipboard: remembered as an implicit
/// grant. Filled in by E6; nothing is recorded yet.
pub fn record_implicit(_daemon: &Arc<Daemon>, _peer: &PublicKey) {}
