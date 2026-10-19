//! Device keys and records. Spec chapter 02 and the `device` schema under
//! `schemas/`. A device is identified by its key; there is no separate id
//! (ADR 0034).
//!
//! A [`DeviceRecord`] is a signed fact carried by the device and gossiped to
//! peers (spec chapter 06). It does not list capabilities; those are live
//! (see [`crate::capability::Manifest`]).

pub use crate::key::DeviceKey;

/// Whether a device record describes the device itself or one person's
/// account on it. Spec chapter 10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    /// The device itself: its own key, a system-level daemon, owner of the
    /// device-scoped capabilities.
    Principal,
    /// One person's account on a device: member of that person's device set,
    /// owner of their user-scoped capabilities there. A personal device is a
    /// single binding.
    Binding,
}

/// Platform family a device runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Platform {
    /// A Linux desktop or laptop.
    Linux,
    /// A macOS desktop or laptop.
    MacOs,
    /// An Android phone or tablet.
    Android,
    /// An iPhone or iPad, reached through BLE accessory profiles.
    Ios,
    /// A headless home hub.
    Hub,
    /// Anything else.
    Other,
}

/// Whether a device belongs to a person or to the group. Spec chapter 02
/// section 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Registration {
    /// Owned by one or more people; the owner set is master keys.
    Personal,
    /// Owned by the group; the owner set is the group key and the device is
    /// listed in the membership document. A tablet, television, hub or lamp.
    Group,
}

/// In-memory view of an `org.ligatureprotocol.device` record.
///
/// Key material, owner set and role signatures are deliberately absent from
/// this scaffold; they will be added once the ceremony and role signatures in
/// `lp-identity` are implemented.
#[derive(Debug, Clone)]
pub struct DeviceRecord {
    /// The device's key, which is its identifier.
    pub key: DeviceKey,
    /// Principal or binding.
    pub kind: DeviceKind,
    /// Display name: the host name by default, overridable by the user.
    pub display_name: String,
    /// Platform family.
    pub platform: Platform,
}
