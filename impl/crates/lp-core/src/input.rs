//! The input capability: descriptor params and the messages of a drive. Spec
//! chapter 05 section 3g; `schemas/capability.input.schema.json`; milestone 7.
//!
//! A drive is one stream opened by the controller: `enter`, `ready` back,
//! `events` batches, until `leave` from the driven device or `release` from
//! either side. Keys and buttons are Linux evdev codes; positions are
//! fractions of the driven screen.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::capability::{Descriptor, Scope};

pub mod keyid;

/// Type name of the input capability, and the purpose of its streams.
pub const INPUT_TYPE: &str = "org.ligatureprotocol.capability.input";

/// The evdev code of the left Control key.
pub const KEY_LEFTCTRL: u16 = 29;
/// The evdev code of the right Control key.
pub const KEY_RIGHTCTRL: u16 = 97;
/// The evdev code of the left mouse button.
pub const BTN_LEFT: u16 = 272;
/// The evdev code of the right mouse button.
pub const BTN_RIGHT: u16 = 273;
/// The evdev code of the middle mouse button.
pub const BTN_MIDDLE: u16 = 274;

/// The input descriptor's `params`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InputParams {
    /// The device injects the events it is sent.
    pub accepts: bool,
    /// The device captures its own keyboard and mouse and sends them.
    pub provides: bool,
    /// The device takes an absolute entry and `position` events.
    pub absolute: bool,
    /// The device types a `text` event's string.
    pub text: bool,
}

impl InputParams {
    /// Reads the params of an input descriptor; `None` if it is not one.
    /// Missing fields are false, as the schema's defaults say.
    #[must_use]
    pub fn from_descriptor(descriptor: &Descriptor) -> Option<Self> {
        if descriptor.capability_type.as_str() != INPUT_TYPE {
            return None;
        }
        let mut params = Self::default();
        if let Some(ciborium::Value::Map(entries)) = &descriptor.params {
            for (k, v) in entries {
                match (k.as_text(), v) {
                    (Some("accepts"), ciborium::Value::Bool(b)) => params.accepts = *b,
                    (Some("provides"), ciborium::Value::Bool(b)) => params.provides = *b,
                    (Some("absolute"), ciborium::Value::Bool(b)) => params.absolute = *b,
                    (Some("text"), ciborium::Value::Bool(b)) => params.text = *b,
                    _ => {}
                }
            }
        }
        Some(params)
    }

    /// Whether the device offers either direction, and so advertises at all.
    #[must_use]
    pub const fn offers_anything(&self) -> bool {
        self.accepts || self.provides
    }
}

/// The descriptor a device advertises for input: device-scoped, offered by
/// the binding standing in for the principal (ADR 0040).
#[must_use]
pub fn input_descriptor(params: InputParams) -> Descriptor {
    let flag = |name: &str, value: bool| {
        (
            ciborium::Value::Text(name.to_owned()),
            ciborium::Value::Bool(value),
        )
    };
    let mut d = Descriptor::new(INPUT_TYPE, Scope::Device);
    d.params = Some(ciborium::Value::Map(vec![
        flag("accepts", params.accepts),
        flag("provides", params.provides),
        flag("absolute", params.absolute),
        flag("text", params.text),
    ]));
    d
}

/// A part of the driven device's whole screen, as fractions of it: where one
/// display lies in a layout of several. A viewer driving a shared display
/// sends positions as fractions of that display, and the driven device maps
/// them into its whole screen through the share's region.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Region {
    /// Left, from the whole screen's left.
    pub x: f64,
    /// Top, from the whole screen's top.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl Region {
    /// The whole screen.
    pub const WHOLE: Self = Self {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    /// A point given as fractions of this region, as fractions of the whole.
    #[must_use]
    pub fn map(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (self.x + x.clamp(0.0, 1.0) * self.width).clamp(0.0, 1.0),
            (self.y + y.clamp(0.0, 1.0) * self.height).clamp(0.0, 1.0),
        )
    }
}

/// An edge of a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    /// The left edge.
    Left,
    /// The right edge.
    Right,
    /// The top edge.
    Top,
    /// The bottom edge.
    Bottom,
}

impl Edge {
    /// Every edge, in the order the layout lists them.
    pub const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Top, Self::Bottom];

    /// The edge the pointer arrives by on the next screen when it leaves by
    /// this one: off the right of one screen is onto the left of the next.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    /// The edge's name on the wire and on the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

impl fmt::Display for Edge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Edge {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "top" => Ok(Self::Top),
            "bottom" => Ok(Self::Bottom),
            other => Err(Error::Decode(format!(
                "{other}: an edge is left, right, top or bottom"
            ))),
        }
    }
}

/// One input event, told apart by `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum InputEvent {
    /// Relative pointer motion, in the driven screen's pixels once the
    /// controller has scaled it.
    Motion {
        /// Horizontal, positive to the right.
        dx: f64,
        /// Vertical, positive downwards.
        dy: f64,
    },
    /// An absolute pointer position as fractions of the driven screen.
    Position {
        /// Across, from the left.
        x: f64,
        /// Down, from the top.
        y: f64,
    },
    /// A pointer button.
    Button {
        /// The evdev button code.
        code: u16,
        /// Pressed or released.
        pressed: bool,
    },
    /// Scrolling.
    Scroll {
        /// Horizontal.
        dx: f64,
        /// Vertical.
        dy: f64,
        /// Whether `dx` and `dy` count wheel steps rather than pixels.
        #[serde(default)]
        discrete: bool,
    },
    /// A key: where it is, and on a press what it means (ADR 0043).
    Key {
        /// The evdev code of the physical key; a release is matched to its
        /// press by it.
        code: u16,
        /// Pressed or released.
        pressed: bool,
        /// What the key means on the controller's layout, as a Synergy key
        /// ID ([`keyid`]); without it the driven device places the key by
        /// `code`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<keyid::KeyId>,
        /// The controller's modifier mask at the event ([`keyid::mask`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mods: Option<u32>,
    },
    /// A string to type as characters.
    Text {
        /// The characters.
        text: String,
    },
}

impl InputEvent {
    /// A key by position alone, as a controller without a layout sends it.
    #[must_use]
    pub const fn key(code: u16, pressed: bool) -> Self {
        Self::Key {
            code,
            pressed,
            id: None,
            mods: None,
        }
    }
}

/// The pointer arrives on the driven device.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Enter {
    /// The edge of the driven screen the pointer crosses, for an edge entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<Edge>,
    /// Where along that edge, from its top or left end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    /// Across the screen from the left, for an absolute entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    /// Down the screen from the top, for an absolute entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
}

impl Enter {
    /// An entry across `edge` at `at` along it.
    #[must_use]
    pub fn edge(edge: Edge, at: f64) -> Self {
        Self {
            edge: Some(edge),
            at: Some(at.clamp(0.0, 1.0)),
            x: None,
            y: None,
        }
    }

    /// An absolute entry at a point on the screen.
    #[must_use]
    pub fn absolute(x: f64, y: f64) -> Self {
        Self {
            edge: None,
            at: None,
            x: Some(x.clamp(0.0, 1.0)),
            y: Some(y.clamp(0.0, 1.0)),
        }
    }

    /// Whether this is an absolute entry (true), an edge entry (false), or
    /// neither (`None`).
    #[must_use]
    pub fn is_absolute(&self) -> Option<bool> {
        match (self.edge, self.at, self.x, self.y) {
            (Some(_), Some(_), _, _) => Some(false),
            (_, _, Some(_), Some(_)) => Some(true),
            _ => None,
        }
    }

    /// Where the pointer starts on the driven screen, as fractions.
    #[must_use]
    pub fn start(&self) -> Option<(f64, f64)> {
        match (self.edge, self.at, self.x, self.y) {
            (Some(edge), Some(at), _, _) => Some(match edge {
                Edge::Left => (0.0, at),
                Edge::Right => (1.0, at),
                Edge::Top => (at, 0.0),
                Edge::Bottom => (at, 1.0),
            }),
            (_, _, Some(x), Some(y)) => Some((x, y)),
            _ => None,
        }
    }
}

/// The driven device's answer to `enter`: its screen's size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
}

/// A batch of events, one input frame's worth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Events {
    /// The events, in order.
    pub events: Vec<InputEvent>,
    /// Present when the controller measures this batch's round trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

/// The driven device has injected the batch that carried `seq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Injected {
    /// The batch's `seq`.
    pub seq: u64,
}

/// The pointer reached the edge it entered by.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Leave {
    /// The edge, the one `enter` named.
    pub edge: Edge,
    /// Where along it.
    pub at: f64,
}

/// One message on the input stream, told apart by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InputMessage {
    /// The first message of a drive, from the controller.
    #[serde(rename = "org.ligatureprotocol.capability.input.enter")]
    Enter(Enter),
    /// The driven device's answer.
    #[serde(rename = "org.ligatureprotocol.capability.input.ready")]
    Ready(Ready),
    /// A batch of events, from the controller.
    #[serde(rename = "org.ligatureprotocol.capability.input.events")]
    Events(Events),
    /// A batch was injected, from the driven device.
    #[serde(rename = "org.ligatureprotocol.capability.input.injected")]
    Injected(Injected),
    /// The pointer left by its entry edge, from the driven device.
    #[serde(rename = "org.ligatureprotocol.capability.input.leave")]
    Leave(Leave),
    /// Lift everything and end the drive, from either side.
    #[serde(rename = "org.ligatureprotocol.capability.input.release")]
    Release,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(message: &InputMessage) -> InputMessage {
        let mut bytes = Vec::new();
        ciborium::into_writer(message, &mut bytes).unwrap();
        ciborium::from_reader(bytes.as_slice()).unwrap()
    }

    #[test]
    fn messages_round_trip_through_cbor_with_their_type_names() {
        let messages = [
            InputMessage::Enter(Enter::edge(Edge::Left, 0.4)),
            InputMessage::Enter(Enter::absolute(0.5, 0.25)),
            InputMessage::Ready(Ready {
                width: 2560,
                height: 1440,
            }),
            InputMessage::Events(Events {
                events: vec![
                    InputEvent::Motion { dx: 3.5, dy: -1.0 },
                    InputEvent::Position { x: 0.1, y: 0.9 },
                    InputEvent::Button {
                        code: BTN_LEFT,
                        pressed: true,
                    },
                    InputEvent::Scroll {
                        dx: 0.0,
                        dy: -1.0,
                        discrete: true,
                    },
                    InputEvent::key(30, false),
                    InputEvent::Text {
                        text: "héllo".to_owned(),
                    },
                ],
                seq: Some(7),
            }),
            InputMessage::Injected(Injected { seq: 7 }),
            InputMessage::Leave(Leave {
                edge: Edge::Left,
                at: 0.7,
            }),
            InputMessage::Release,
        ];
        for m in &messages {
            assert_eq!(&round_trip(m), m);
        }
        let mut bytes = Vec::new();
        ciborium::into_writer(&InputMessage::Release, &mut bytes).unwrap();
        let value: ciborium::Value = ciborium::from_reader(bytes.as_slice()).unwrap();
        let ciborium::Value::Map(entries) = value else {
            panic!("a message is a map")
        };
        assert_eq!(
            entries[0],
            (
                ciborium::Value::Text("type".into()),
                ciborium::Value::Text("org.ligatureprotocol.capability.input.release".into())
            )
        );
    }

    #[test]
    fn events_use_kind_and_the_schema_s_field_names() {
        let json = serde_json::to_value(InputEvent::Scroll {
            dx: 1.0,
            dy: 2.0,
            discrete: false,
        })
        .unwrap();
        assert_eq!(json["kind"], "scroll");
        assert_eq!(json["dx"], 1.0);
        let key: InputEvent =
            serde_json::from_str(r#"{"kind":"key","code":30,"pressed":true}"#).unwrap();
        assert_eq!(key, InputEvent::key(30, true));
    }

    #[test]
    fn params_round_trip_through_the_descriptor() {
        let params = InputParams {
            accepts: true,
            provides: false,
            absolute: true,
            text: true,
        };
        let d = input_descriptor(params);
        assert_eq!(d.scope, Scope::Device);
        assert_eq!(InputParams::from_descriptor(&d), Some(params));
        let bare = Descriptor::new(INPUT_TYPE, Scope::Device);
        assert_eq!(
            InputParams::from_descriptor(&bare),
            Some(InputParams::default())
        );
        let other = Descriptor::new(crate::files::FILES_TYPE, Scope::User);
        assert_eq!(InputParams::from_descriptor(&other), None);
    }

    #[test]
    fn edges_and_entries() {
        assert_eq!(Edge::Right.opposite(), Edge::Left);
        assert_eq!(Edge::Top.opposite(), Edge::Bottom);
        assert_eq!("bottom".parse::<Edge>().unwrap(), Edge::Bottom);
        assert!("middle".parse::<Edge>().is_err());
        assert_eq!(Enter::edge(Edge::Right, 0.25).start(), Some((1.0, 0.25)));
        assert_eq!(Enter::edge(Edge::Bottom, 0.25).start(), Some((0.25, 1.0)));
        assert_eq!(Enter::absolute(0.3, 0.6).start(), Some((0.3, 0.6)));
        assert_eq!(Enter::edge(Edge::Left, 2.0).at, Some(1.0));
        assert_eq!(Enter::edge(Edge::Left, 0.5).is_absolute(), Some(false));
        assert_eq!(Enter::absolute(0.5, 0.5).is_absolute(), Some(true));
        let neither = Enter {
            edge: Some(Edge::Left),
            at: None,
            x: None,
            y: None,
        };
        assert_eq!(neither.is_absolute(), None);
    }
}
