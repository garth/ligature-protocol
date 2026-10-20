//! The capability model: descriptors, manifests and grants. Spec chapter 05
//! and the `capability` and `grant` schemas under `schemas/`.

use serde::{Deserialize, Serialize};

use crate::wire::{Timestamp, TypeName};

/// Type name of the manifest message on the `manifest` stream (ADR 0036).
pub const MANIFEST_TYPE: &str = "org.ligatureprotocol.manifest";
/// Type name of the clipboard capability.
pub const CLIPBOARD_TYPE: &str = "org.ligatureprotocol.capability.clipboard";
/// Type name of the clipboard `set` message.
pub const CLIPBOARD_SET_TYPE: &str = "org.ligatureprotocol.capability.clipboard.set";

/// `clipboard.set`: the sender's clipboard now holds this text. Spec chapter
/// 05 section 3b; `schemas/capability.clipboard.schema.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardSet {
    /// Always [`CLIPBOARD_SET_TYPE`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// Content type; `text/plain` in this version.
    #[serde(default = "text_plain")]
    pub mime: String,
    /// The content.
    pub text: String,
    /// When the sender's clipboard took this value; the newest wins.
    pub set_at: Timestamp,
}

fn text_plain() -> String {
    "text/plain".to_owned()
}

impl ClipboardSet {
    /// A `set` of `text` taken now.
    #[must_use]
    pub fn now(text: &str) -> Self {
        Self {
            message_type: TypeName::new(CLIPBOARD_SET_TYPE),
            mime: text_plain(),
            text: text.to_owned(),
            set_at: Timestamp::now(),
        }
    }
}

/// The descriptor this device advertises for its clipboard.
#[must_use]
pub fn clipboard_descriptor(max_bytes: u64) -> Descriptor {
    let mut d = Descriptor::new(CLIPBOARD_TYPE, Scope::User);
    d.params = Some(ciborium::Value::Map(vec![
        (
            ciborium::Value::Text("maxBytes".into()),
            ciborium::Value::Integer(max_bytes.into()),
        ),
        (
            ciborium::Value::Text("mimeTypes".into()),
            ciborium::Value::Array(vec![ciborium::Value::Text("text/plain".into())]),
        ),
    ]));
    d
}

/// A reverse-DNS type name, such as `org.ligatureprotocol.capability.clipboard`
/// for a capability or `org.ligatureprotocol.device` for a fact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityType(pub String);

impl CapabilityType {
    /// The string form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Owned by a user-device binding: clipboard, notifications, personal files.
    /// Implicitly available to the same person's other devices.
    User,
    /// Owned by the device principal: display, speakers, sensors, physical
    /// input. Cross-identity use is approved by the person present on the
    /// device, else by device policy.
    Device,
}

/// One capability offered by one principal or binding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Descriptor {
    /// The capability type.
    #[serde(rename = "type")]
    pub capability_type: CapabilityType,
    /// Whether this is a user-scoped or device-scoped capability.
    pub scope: Scope,
    /// Version of the type's operation set the device implements.
    #[serde(default = "one")]
    pub version: u32,
    /// Optional human-readable label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Type-specific parameters, per the capability type's own schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<ciborium::Value>,
}

const fn one() -> u32 {
    1
}

impl Descriptor {
    /// A descriptor with no label or parameters.
    #[must_use]
    pub fn new(capability_type: &str, scope: Scope) -> Self {
        Self {
            capability_type: CapabilityType(capability_type.to_owned()),
            scope,
            version: 1,
            label: None,
            params: None,
        }
    }
}

/// The full list of capabilities a peer currently offers. Spec chapter 05
/// section 2a.
///
/// Sent after session authentication on the `manifest` stream and replaced by
/// later manifests from the same peer. When the session ends every capability
/// in it is withdrawn. Never persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Always [`MANIFEST_TYPE`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// Capabilities offered right now.
    pub capabilities: Vec<Descriptor>,
    /// When this manifest was made.
    pub issued_at: Timestamp,
}

impl Manifest {
    /// A manifest of `capabilities`, issued now.
    #[must_use]
    pub fn new(capabilities: Vec<Descriptor>) -> Self {
        Self {
            message_type: TypeName::new(MANIFEST_TYPE),
            capabilities,
            issued_at: Timestamp::now(),
        }
    }

    /// An empty manifest.
    #[must_use]
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }
}

// Grants are facts: see [`crate::facts::GrantFact`].
