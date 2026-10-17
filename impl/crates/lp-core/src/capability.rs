//! The capability model: descriptors, manifests and grants. Spec chapter 05
//! and the `capability` and `grant` schemas under `schemas/`.

/// A reverse-DNS type name for a capability, such as
/// `org.ligatureprotocol.capability.clipboard`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CapabilityType(pub String);

/// The initial capability families. Each will receive its own schema and
/// spec section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Family {
    /// Forwarding and acting on notifications.
    Notifications,
    /// Shared clipboard.
    Clipboard,
    /// File transfer and browsing among a person's own devices.
    Files,
    /// Ad-hoc, AirDrop-like push of text, links or files to a chosen device,
    /// usually another person's, accepted per item by the receiver.
    Share,
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

/// Whether a capability belongs to a person's session on a device or to the
/// device itself. Spec chapter 10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Owned by a user-device binding: clipboard, notifications, personal files.
    /// Implicitly available to the same person's other devices.
    User,
    /// Owned by the device principal: display, speakers, sensors, physical
    /// input. Cross-identity use is approved by the person present on the
    /// device, else by device policy.
    Device,
}

/// One capability offered by one principal.
#[derive(Debug, Clone)]
pub struct Descriptor {
    /// The capability type.
    pub capability_type: CapabilityType,
    /// Whether this is a user-scoped or device-scoped capability.
    pub scope: Scope,
    /// Version of the type's operation set the device implements.
    pub version: u32,
    /// Optional human-readable label.
    pub label: Option<String>,
}

/// The full list of capabilities a peer currently offers. Spec chapter 05
/// section 2a.
///
/// Sent after session authentication and replaced by later manifests from the
/// same peer. When the session ends every capability in it is withdrawn.
/// Never persisted.
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    /// Capabilities offered right now.
    pub capabilities: Vec<Descriptor>,
}

/// How a grant came about. Spec chapter 10 section 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GrantBasis {
    /// Same identity, user-scoped capability: no prompt was needed.
    Implicit,
    /// A person approved it, once or as a standing grant.
    Approval,
    /// Device policy allowed it while nobody was present.
    Policy,
}

/// An explicit authorisation for one device to use a capability of another.
///
/// Expiry and capability-specific restrictions will be added when the grant
/// semantics in spec chapter 05 are settled.
#[derive(Debug, Clone)]
pub struct Grant {
    /// How the grant came about.
    pub basis: GrantBasis,
    /// Device offering the capability.
    pub grantor: crate::device::DeviceId,
    /// Device permitted to use it.
    pub grantee: crate::device::DeviceId,
    /// The granted capability type.
    pub capability_type: CapabilityType,
}
