//! The capability model: descriptors, manifests and grants. Spec chapter 05
//! and the `capability` and `grant` schemas under `schemas/`.

use serde::{Deserialize, Serialize};

use crate::PublicKey;
use crate::wire::{Bytes, Timestamp, TypeName};

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

/// Type name of the notifications capability.
pub const NOTIFICATIONS_TYPE: &str = "org.ligatureprotocol.capability.notifications";
/// Type name of the notifications `post` message.
pub const NOTIFICATIONS_POST_TYPE: &str = "org.ligatureprotocol.capability.notifications.post";
/// Type name of the notifications `icon` message.
pub const NOTIFICATIONS_ICON_TYPE: &str = "org.ligatureprotocol.capability.notifications.icon";
/// Type name of the notifications `dismiss` message.
pub const NOTIFICATIONS_DISMISS_TYPE: &str =
    "org.ligatureprotocol.capability.notifications.dismiss";

/// The notifications descriptor's `params`: which directions the device
/// offers and how large an icon it accepts. Spec chapter 05 section 3c;
/// `schemas/capability.notifications.schema.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationsParams {
    /// The device forwards what its apps post.
    pub emits: bool,
    /// The device shows what it is sent.
    pub displays: bool,
    /// The largest icon, in bytes, accepted in one `icon`; 0 wants none.
    pub max_icon_bytes: u64,
}

impl NotificationsParams {
    /// The default icon bound: 64 KiB.
    pub const DEFAULT_MAX_ICON_BYTES: u64 = 65536;

    /// Reads the params of a notifications descriptor; `None` if it is not
    /// one. Missing fields take the schema's defaults.
    #[must_use]
    pub fn from_descriptor(descriptor: &Descriptor) -> Option<Self> {
        if descriptor.capability_type.as_str() != NOTIFICATIONS_TYPE {
            return None;
        }
        let mut params = Self {
            emits: false,
            displays: true,
            max_icon_bytes: Self::DEFAULT_MAX_ICON_BYTES,
        };
        if let Some(ciborium::Value::Map(entries)) = &descriptor.params {
            for (k, v) in entries {
                match (k.as_text(), v) {
                    (Some("emits"), ciborium::Value::Bool(b)) => params.emits = *b,
                    (Some("displays"), ciborium::Value::Bool(b)) => params.displays = *b,
                    (Some("maxIconBytes"), ciborium::Value::Integer(i)) => {
                        params.max_icon_bytes = u64::try_from(*i).unwrap_or(0);
                    }
                    _ => {}
                }
            }
        }
        Some(params)
    }
}

/// The descriptor a device advertises for notifications.
#[must_use]
pub fn notifications_descriptor(params: NotificationsParams) -> Descriptor {
    let mut d = Descriptor::new(NOTIFICATIONS_TYPE, Scope::User);
    d.params = Some(ciborium::Value::Map(vec![
        (
            ciborium::Value::Text("emits".into()),
            ciborium::Value::Bool(params.emits),
        ),
        (
            ciborium::Value::Text("displays".into()),
            ciborium::Value::Bool(params.displays),
        ),
        (
            ciborium::Value::Text("maxIconBytes".into()),
            ciborium::Value::Integer(params.max_icon_bytes.into()),
        ),
    ]));
    d
}

/// The app that posted a notification, as its platform names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationApp {
    /// The platform's identifier: a package name on Android, the app name
    /// given to the notification service on Linux.
    pub id: String,
    /// The display name.
    pub name: String,
}

/// `notifications.post`: an app on the sender posted or updated this
/// notification. Spec chapter 05 section 3c.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPost {
    /// Chosen by the emitting device, stable for the notification's life.
    pub id: String,
    /// The app.
    pub app: NotificationApp,
    /// The title, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The body, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// When the app posted it on the emitting device.
    pub posted_at: Timestamp,
    /// The hash of the app's icon, sent earlier in this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// `notifications.icon`: an app's icon, once per app per session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationIcon {
    /// SHA-256 of `bytes`, base64url without padding.
    pub hash: String,
    /// `image/png` in this version.
    pub mime: String,
    /// The image.
    pub bytes: Bytes,
}

/// `notifications.dismiss`: the user dismissed this notification on the
/// sender. Sent to the origin and every displaying peer, never forwarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationDismiss {
    /// The binding key of the device that emitted the notification.
    pub origin: PublicKey,
    /// Its id in that device's id space.
    pub id: String,
    /// When the user dismissed it.
    pub dismissed_at: Timestamp,
}

/// One message on the notifications stream, told apart by `type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NotificationMessage {
    /// A post.
    #[serde(rename = "org.ligatureprotocol.capability.notifications.post")]
    Post(NotificationPost),
    /// An icon.
    #[serde(rename = "org.ligatureprotocol.capability.notifications.icon")]
    Icon(NotificationIcon),
    /// A dismissal.
    #[serde(rename = "org.ligatureprotocol.capability.notifications.dismiss")]
    Dismiss(NotificationDismiss),
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
