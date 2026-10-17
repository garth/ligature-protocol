//! Device identifiers and records. Spec chapter 02 and the
//! `org.ligatureprotocol.device` Lexicon.

/// Stable opaque identifier for a device, derived from its signing key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

/// Platform family a device runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Platform {
    /// A Linux desktop or laptop.
    Linux,
    /// An Android phone or tablet.
    Android,
    /// An iPhone or iPad, reached through BLE accessory profiles.
    Ios,
    /// A headless home hub.
    Hub,
    /// Anything else.
    Other,
}

/// In-memory view of an `org.ligatureprotocol.device` record.
///
/// Key material is deliberately absent from this scaffold; it will be added
/// once the cross-signing design in `lp-identity` is implemented.
#[derive(Debug, Clone)]
pub struct DeviceRecord {
    /// The device's identifier.
    pub id: DeviceId,
    /// User-chosen display name.
    pub display_name: String,
    /// Platform family.
    pub platform: Platform,
    /// Capabilities the device currently advertises.
    pub capabilities: Vec<crate::capability::Descriptor>,
}
