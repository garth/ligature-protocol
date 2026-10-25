//! The screen capability: descriptor params and the messages of a share. Spec
//! chapter 05 section 3h; `schemas/capability.screen.schema.json`; milestone 8.
//!
//! A share is one stream opened by the viewer: `displays` to list, `start`
//! answered by `started` or `refused`, `frame` chunks from the sharer,
//! `keyframe` from the viewer, `stop` from either side. No chunk exceeds
//! [`CHUNK_BYTES`], so no message exceeds the bound of ADR 0036.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::capability::{Descriptor, Scope};
use crate::wire::Bytes;

/// Type name of the screen capability, and the purpose of its streams.
pub const SCREEN_TYPE: &str = "org.ligatureprotocol.capability.screen";

/// The largest `bytes` of one `frame` chunk: 256 KiB.
pub const CHUNK_BYTES: usize = 262_144;

/// A video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    /// H.264 as an Annex B byte stream, the parameter sets in every keyframe.
    H264,
}

impl Codec {
    /// The name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::H264 => "h264",
        }
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The screen descriptor's `params`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenParams {
    /// The device captures and encodes a display.
    pub shares: bool,
    /// The device decodes and shows a shared display.
    pub views: bool,
    /// The codecs it encodes or decodes, most preferred first.
    pub codecs: Vec<Codec>,
    /// The widest picture it takes as a viewer.
    pub max_width: u32,
    /// The tallest picture it takes as a viewer.
    pub max_height: u32,
    /// The fastest it takes as a viewer, frames per second.
    pub max_frame_rate: u32,
}

impl Default for ScreenParams {
    fn default() -> Self {
        Self {
            shares: false,
            views: false,
            codecs: vec![Codec::H264],
            max_width: 1920,
            max_height: 1080,
            max_frame_rate: 30,
        }
    }
}

impl ScreenParams {
    /// Reads the params of a screen descriptor; `None` if it is not one.
    /// Missing fields take the schema's defaults.
    #[must_use]
    pub fn from_descriptor(descriptor: &Descriptor) -> Option<Self> {
        if descriptor.capability_type.as_str() != SCREEN_TYPE {
            return None;
        }
        let mut params = Self::default();
        let int = |i: &ciborium::value::Integer| u32::try_from(*i).ok().filter(|v| *v >= 1);
        if let Some(ciborium::Value::Map(entries)) = &descriptor.params {
            for (k, v) in entries {
                match (k.as_text(), v) {
                    (Some("shares"), ciborium::Value::Bool(b)) => params.shares = *b,
                    (Some("views"), ciborium::Value::Bool(b)) => params.views = *b,
                    (Some("codecs"), ciborium::Value::Array(items)) => {
                        params.codecs = items
                            .iter()
                            .filter_map(|c| match c.as_text() {
                                Some("h264") => Some(Codec::H264),
                                _ => None,
                            })
                            .collect();
                    }
                    (Some("maxWidth"), ciborium::Value::Integer(i)) => {
                        if let Some(v) = int(i) {
                            params.max_width = v;
                        }
                    }
                    (Some("maxHeight"), ciborium::Value::Integer(i)) => {
                        if let Some(v) = int(i) {
                            params.max_height = v;
                        }
                    }
                    (Some("maxFrameRate"), ciborium::Value::Integer(i)) => {
                        if let Some(v) = int(i) {
                            params.max_frame_rate = v;
                        }
                    }
                    _ => {}
                }
            }
        }
        Some(params)
    }

    /// Whether the device offers either direction, and so advertises at all.
    #[must_use]
    pub const fn offers_anything(&self) -> bool {
        self.shares || self.views
    }
}

/// The descriptor a device advertises for screen: device-scoped, offered by
/// the binding standing in for the principal (ADR 0040).
#[must_use]
pub fn screen_descriptor(params: &ScreenParams) -> Descriptor {
    use ciborium::Value;
    let mut d = Descriptor::new(SCREEN_TYPE, Scope::Device);
    d.params = Some(Value::Map(vec![
        (Value::Text("shares".into()), Value::Bool(params.shares)),
        (Value::Text("views".into()), Value::Bool(params.views)),
        (
            Value::Text("codecs".into()),
            Value::Array(
                params
                    .codecs
                    .iter()
                    .map(|c| Value::Text(c.as_str().to_owned()))
                    .collect(),
            ),
        ),
        (
            Value::Text("maxWidth".into()),
            Value::Integer(params.max_width.into()),
        ),
        (
            Value::Text("maxHeight".into()),
            Value::Integer(params.max_height.into()),
        ),
        (
            Value::Text("maxFrameRate".into()),
            Value::Integer(params.max_frame_rate.into()),
        ),
    ]));
    d
}

/// The largest picture of a `width` by `height` display that fits within
/// `max_width` by `max_height`, keeping the aspect and never scaling up.
/// Dimensions are even, as encoders want.
#[must_use]
pub fn fit(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let (width, height) = (width.max(1), height.max(1));
    let scale = (f64::from(max_width.max(1)) / f64::from(width))
        .min(f64::from(max_height.max(1)) / f64::from(height))
        .min(1.0);
    let even = |v: f64| {
        let v = v.floor() as u32;
        (v - (v % 2)).max(2)
    };
    (
        even(f64::from(width) * scale),
        even(f64::from(height) * scale),
    )
}

/// One display a sharer offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Display {
    /// The sharer's name for it, stable for the session.
    pub id: String,
    /// What the platform calls it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its width in pixels.
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
    /// Whether a `start` without an id shares it.
    #[serde(default)]
    pub primary: bool,
    /// Its pixels per logical point (ADR 0045), when the platform says; see
    /// [`Display::scale`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
}

impl Display {
    /// Its pixels per logical point: what it reported, or 1 when it reported
    /// nothing usable.
    #[must_use]
    pub fn scale(&self) -> f64 {
        self.scale
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(1.0)
    }
}

/// `displays`: a request without a list, an answer with one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Displays {
    /// The sharer's displays; present only in the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub displays: Option<Vec<Display>>,
}

/// `start`: what the viewer wants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Start {
    /// The display, or the primary one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// The codec.
    pub codec: Codec,
    /// The widest picture wanted.
    pub width: u32,
    /// The tallest picture wanted.
    pub height: u32,
    /// The frame rate wanted.
    pub frame_rate: u32,
}

/// `started`: what the sharer settled on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    /// The display shared.
    pub display: String,
    /// The codec.
    pub codec: Codec,
    /// The picture's width.
    pub width: u32,
    /// The picture's height.
    pub height: u32,
    /// The frame rate paced to.
    pub frame_rate: u32,
}

/// Why a share did not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefuseReason {
    /// The platform's consent was not given, or not in time.
    Consent,
    /// No encoder for the codec asked.
    NoEncoder,
    /// No display with that id.
    NoDisplay,
    /// A share of this display already runs.
    Busy,
}

impl fmt::Display for RefuseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Consent => "consent",
            Self::NoEncoder => "no-encoder",
            Self::NoDisplay => "no-display",
            Self::Busy => "busy",
        })
    }
}

/// `refused`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    /// Why.
    pub reason: RefuseReason,
}

/// `frame`: one chunk of one encoded frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    /// The frame's number from the start of the share.
    pub seq: u64,
    /// When it was captured, microseconds since the share started.
    pub at: u64,
    /// Whether it is a keyframe.
    pub key: bool,
    /// Whether this chunk completes the frame.
    pub last: bool,
    /// The chunk, at most [`CHUNK_BYTES`].
    pub bytes: Bytes,
}

/// Why a share ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopReason {
    /// The viewer closed.
    Viewer,
    /// The sharer's user stopped it.
    Sharer,
    /// The platform withdrew its consent.
    Consent,
    /// Capture or encoding failed.
    Error,
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Viewer => "viewer",
            Self::Sharer => "sharer",
            Self::Consent => "consent",
            Self::Error => "error",
        })
    }
}

/// `stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {
    /// Why.
    pub reason: StopReason,
}

/// One message on the screen stream, told apart by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ScreenMessage {
    /// The displays, asked for or listed.
    #[serde(rename = "org.ligatureprotocol.capability.screen.displays")]
    Displays(Displays),
    /// From the viewer: share this.
    #[serde(rename = "org.ligatureprotocol.capability.screen.start")]
    Start(Start),
    /// From the sharer: the share runs.
    #[serde(rename = "org.ligatureprotocol.capability.screen.started")]
    Started(Started),
    /// From the sharer: it does not.
    #[serde(rename = "org.ligatureprotocol.capability.screen.refused")]
    Refused(Refused),
    /// From the sharer: a chunk of a frame.
    #[serde(rename = "org.ligatureprotocol.capability.screen.frame")]
    Frame(Frame),
    /// From the viewer: a keyframe, please.
    #[serde(rename = "org.ligatureprotocol.capability.screen.keyframe")]
    Keyframe,
    /// From either side: the share ends.
    #[serde(rename = "org.ligatureprotocol.capability.screen.stop")]
    Stop(Stop),
}

/// Splits one encoded frame into `frame` messages of at most [`CHUNK_BYTES`]
/// each, the last marked. An empty frame is one empty chunk.
#[must_use]
pub fn chunk_frame(seq: u64, at: u64, key: bool, bytes: &[u8]) -> Vec<Frame> {
    if bytes.is_empty() {
        return vec![Frame {
            seq,
            at,
            key,
            last: true,
            bytes: Bytes(Vec::new()),
        }];
    }
    let count = bytes.len().div_ceil(CHUNK_BYTES);
    bytes
        .chunks(CHUNK_BYTES)
        .enumerate()
        .map(|(i, chunk)| Frame {
            seq,
            at,
            key,
            last: i + 1 == count,
            bytes: Bytes(chunk.to_vec()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_round_trip_through_the_descriptor() {
        let params = ScreenParams {
            shares: true,
            views: true,
            codecs: vec![Codec::H264],
            max_width: 1280,
            max_height: 720,
            max_frame_rate: 60,
        };
        let d = screen_descriptor(&params);
        assert_eq!(d.scope, Scope::Device);
        assert_eq!(ScreenParams::from_descriptor(&d), Some(params));
        let bare = Descriptor::new(SCREEN_TYPE, Scope::Device);
        assert_eq!(
            ScreenParams::from_descriptor(&bare),
            Some(ScreenParams::default())
        );
        assert_eq!(
            ScreenParams::from_descriptor(&Descriptor::new(
                crate::input::INPUT_TYPE,
                Scope::Device
            )),
            None
        );
    }

    #[test]
    fn fitting_keeps_the_aspect_and_never_scales_up() {
        assert_eq!(fit(2560, 1440, 1280, 720), (1280, 720));
        assert_eq!(fit(2560, 1440, 1920, 1080), (1920, 1080));
        assert_eq!(fit(1920, 1080, 1280, 1280), (1280, 720));
        assert_eq!(fit(1080, 2400, 1920, 1080), (486, 1080));
        assert_eq!(fit(800, 600, 1920, 1080), (800, 600));
        assert_eq!(fit(1, 1, 1, 1), (2, 2));
    }

    #[test]
    fn chunks_are_bounded_and_the_last_is_marked() {
        let bytes = vec![7u8; CHUNK_BYTES * 2 + 5];
        let chunks = chunk_frame(3, 1000, true, &bytes);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.seq == 3 && c.at == 1000 && c.key));
        assert_eq!(chunks[0].bytes.0.len(), CHUNK_BYTES);
        assert_eq!(chunks[2].bytes.0.len(), 5);
        assert_eq!(
            chunks.iter().map(|c| c.last).collect::<Vec<_>>(),
            vec![false, false, true]
        );
        let joined: Vec<u8> = chunks.into_iter().flat_map(|c| c.bytes.0).collect();
        assert_eq!(joined, bytes);
        let exact = chunk_frame(0, 0, false, &vec![1u8; CHUNK_BYTES]);
        assert_eq!(exact.len(), 1);
        assert!(exact[0].last);
        assert_eq!(chunk_frame(0, 0, false, &[]).len(), 1);
    }

    #[test]
    fn messages_round_trip_through_cbor() {
        let messages = [
            ScreenMessage::Displays(Displays::default()),
            ScreenMessage::Displays(Displays {
                displays: Some(vec![Display {
                    id: "DP-1".into(),
                    name: Some("Dell".into()),
                    width: 2560,
                    height: 1440,
                    primary: true,
                    scale: Some(1.5),
                }]),
            }),
            ScreenMessage::Start(Start {
                display: None,
                codec: Codec::H264,
                width: 1280,
                height: 720,
                frame_rate: 30,
            }),
            ScreenMessage::Started(Started {
                display: "DP-1".into(),
                codec: Codec::H264,
                width: 1280,
                height: 720,
                frame_rate: 30,
            }),
            ScreenMessage::Refused(Refused {
                reason: RefuseReason::NoEncoder,
            }),
            ScreenMessage::Frame(Frame {
                seq: 1,
                at: 33_333,
                key: true,
                last: false,
                bytes: Bytes(vec![0, 0, 0, 1, 0x67]),
            }),
            ScreenMessage::Keyframe,
            ScreenMessage::Stop(Stop {
                reason: StopReason::Consent,
            }),
        ];
        for m in &messages {
            let mut bytes = Vec::new();
            ciborium::into_writer(m, &mut bytes).unwrap();
            let back: ScreenMessage = ciborium::from_reader(bytes.as_slice()).unwrap();
            assert_eq!(&back, m);
        }
        let json = serde_json::to_value(&messages[4]).unwrap();
        assert_eq!(json["reason"], "no-encoder");
        let json = serde_json::to_value(&messages[2]).unwrap();
        assert_eq!(json["frameRate"], 30);
        assert_eq!(json["codec"], "h264");
    }
}
