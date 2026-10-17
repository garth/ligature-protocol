//! The capability model: descriptors, requests and grants. Spec chapter 05 and
//! the `org.ligatureprotocol.capability` / `org.ligatureprotocol.grant`
//! Lexicons.

/// An atproto NSID naming a capability type, such as
/// `org.ligatureprotocol.capability.clipboard`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CapabilityType(pub String);

/// The initial capability families. Each will receive its own Lexicon and
/// spec section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Family {
    /// Forwarding and acting on notifications.
    Notifications,
    /// Shared clipboard.
    Clipboard,
    /// File transfer and browsing.
    Files,
    /// Keyboard, mouse and touch input to a remote device.
    RemoteInput,
    /// Play, pause, seek, volume.
    MediaControl,
    /// Calls and SMS.
    Telephony,
    /// Using a peer's screen as a display.
    Displays,
    /// Reading a peer's sensors.
    Sensors,
}

/// One capability offered by one device.
#[derive(Debug, Clone)]
pub struct Descriptor {
    /// The capability type.
    pub capability_type: CapabilityType,
    /// Version of the type's operation set the device implements.
    pub version: u32,
    /// Optional human-readable label.
    pub label: Option<String>,
}

/// An explicit authorisation for one device to use a capability of another.
///
/// Scope and expiry will be added when the grant semantics in spec chapter 05
/// are settled.
#[derive(Debug, Clone)]
pub struct Grant {
    /// Device offering the capability.
    pub grantor: crate::device::DeviceId,
    /// Device permitted to use it.
    pub grantee: crate::device::DeviceId,
    /// The granted capability type.
    pub capability_type: CapabilityType,
}
