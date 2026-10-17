//! Encryption layer: an MLS (RFC 9420) group per device set.
//!
//! Implements spec chapter 03 (ADR 0004). This crate wraps an MLS
//! implementation (planned: OpenMLS) and exposes group lifecycle operations
//! (create, add device, remove device), application message encryption and
//! exporter secrets for bulk transfer.
//!
//! **Status:** scaffold. No MLS dependency is pulled in yet.

pub mod group;
