//! The camera capability: a camera of one device, with its microphone, sent
//! live to another. Spec chapter 05 section 3i;
//! `schemas/capability.camera.schema.json`; milestone 10.
//!
//! The family is the screen family under its own type names: the params, the
//! formats, `frame`, `audio`, `keyframe`, `stop` and `refused` are the screen
//! family's, `start` and `started` name a `camera` where the screen's name a
//! display, and the listing is `cameras`. So an implementation runs one
//! machine for both, and [`CameraMessage::into_screen`] and
//! [`CameraMessage::from_screen`] carry a message between the two sets of
//! names.

use serde::{Deserialize, Serialize};

use crate::capability::Descriptor;
use crate::screen::{
    Audio, AudioFormat, Displays, Format, Frame, Offer, Refused, ScreenMessage, ScreenParams,
    Source, SourceKind, Start, Started, Stop, descriptor_of, known_audio_formats, known_formats,
};

/// Type name of the camera capability, and the purpose of its streams.
pub const CAMERA_TYPE: &str = "org.ligatureprotocol.capability.camera";

/// The camera descriptor's params, the screen family's with `presents`;
/// `None` if the descriptor is not a camera's.
#[must_use]
pub fn camera_params(descriptor: &Descriptor) -> Option<ScreenParams> {
    ScreenParams::from_descriptor_of(descriptor, CAMERA_TYPE)
}

/// The descriptor a device advertises for its cameras: device-scoped, as the
/// screen's is.
#[must_use]
pub fn camera_descriptor(params: &ScreenParams) -> Descriptor {
    descriptor_of(CAMERA_TYPE, params)
}

/// Which way a camera looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Facing {
    /// Towards the person at the screen.
    Front,
    /// Away from them.
    Back,
    /// Plugged in, or of a direction the platform does not say.
    #[default]
    External,
}

impl Facing {
    /// The name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Back => "back",
            Self::External => "external",
        }
    }
}

impl std::fmt::Display for Facing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Facing {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Facing {
    /// A value this device does not know is `external`, as the schema says.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "front" => Self::Front,
            "back" => Self::Back,
            _ => Self::External,
        })
    }
}

/// One camera a sharer offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Camera {
    /// The sharer's name for it, stable for the session.
    pub id: String,
    /// What the platform calls it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Which way it looks.
    #[serde(default)]
    pub facing: Facing,
    /// The widest picture it captures, in pixels.
    pub width: u32,
    /// The tallest picture it captures, in pixels.
    pub height: u32,
    /// The frame rates it captures at, where the platform says them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frame_rates: Vec<u32>,
}

/// `cameras`: a request without a list, an answer with one, the first being
/// the camera a `start` without an id shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Cameras {
    /// The sharer's cameras; present only in the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cameras: Option<Vec<Camera>>,
}

/// What a cast of a camera shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraSource {
    /// Always `camera`.
    pub kind: SourceKind,
    /// The caster's name for the camera.
    pub id: String,
    /// What the platform calls it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The widest picture it captures.
    pub width: u32,
    /// The tallest picture it captures.
    pub height: u32,
}

/// `offer`: the first message of a cast of a camera.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraOffer {
    /// The camera.
    pub source: CameraSource,
    /// The picture formats the caster encodes, most preferred first.
    #[serde(deserialize_with = "known_formats")]
    pub formats: Vec<Format>,
    /// The fastest the caster paces to.
    pub max_frame_rate: u32,
    /// The audio formats the caster can send from its microphone.
    #[serde(
        default,
        deserialize_with = "known_audio_formats",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub audio: Vec<AudioFormat>,
    /// Whether the caster asks for the camera to be presented to the
    /// receiver's programs rather than shown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub present: bool,
}

/// `start`: what the viewer wants of a camera.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraStart {
    /// The camera, or the first listed when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// The formats the viewer takes, most preferred first.
    #[serde(deserialize_with = "known_formats")]
    pub formats: Vec<Format>,
    /// The widest picture wanted.
    pub width: u32,
    /// The tallest picture wanted.
    pub height: u32,
    /// The frame rate wanted.
    pub frame_rate: u32,
    /// The audio formats the viewer takes; empty for no microphone.
    #[serde(
        default,
        deserialize_with = "known_audio_formats",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub audio: Vec<AudioFormat>,
}

/// `started`: what the sharer settled on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraStarted {
    /// The camera shared.
    pub camera: String,
    /// The format picked.
    pub format: Format,
    /// The audio format picked, when the microphone comes too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioFormat>,
    /// The picture's width.
    pub width: u32,
    /// The picture's height.
    pub height: u32,
    /// The frame rate paced to.
    pub frame_rate: u32,
}

/// One message on a camera stream, told apart by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CameraMessage {
    /// The cameras, asked for or listed.
    #[serde(rename = "org.ligatureprotocol.capability.camera.cameras")]
    Cameras(Cameras),
    /// From the sharer, opening a cast: here is the camera I will share.
    #[serde(rename = "org.ligatureprotocol.capability.camera.offer")]
    Offer(CameraOffer),
    /// From the viewer, or a receiver taking an offer: share this.
    #[serde(rename = "org.ligatureprotocol.capability.camera.start")]
    Start(CameraStart),
    /// From the sharer: the share runs.
    #[serde(rename = "org.ligatureprotocol.capability.camera.started")]
    Started(CameraStarted),
    /// From the side that was asked: it does not.
    #[serde(rename = "org.ligatureprotocol.capability.camera.refused")]
    Refused(Refused),
    /// From the sharer: a chunk of a frame.
    #[serde(rename = "org.ligatureprotocol.capability.camera.frame")]
    Frame(Frame),
    /// From the sharer: a packet of the microphone's sound.
    #[serde(rename = "org.ligatureprotocol.capability.camera.audio")]
    Audio(Audio),
    /// From the viewer: a keyframe, please.
    #[serde(rename = "org.ligatureprotocol.capability.camera.keyframe")]
    Keyframe,
    /// From either side: the share ends.
    #[serde(rename = "org.ligatureprotocol.capability.camera.stop")]
    Stop(Stop),
}

impl CameraMessage {
    /// The message in the screen family's shapes, which the shared machine
    /// runs on: a camera is a display by another name there, and a `cameras`
    /// message, asked or answered, the listing request (the answer's list is
    /// read from the camera message itself by whoever asked).
    #[must_use]
    pub fn into_screen(self) -> ScreenMessage {
        match self {
            Self::Cameras(_) => ScreenMessage::Displays(Displays::default()),
            Self::Offer(o) => ScreenMessage::Offer(Offer {
                source: Source {
                    kind: SourceKind::Camera,
                    id: o.source.id,
                    name: o.source.name,
                    app: None,
                    width: o.source.width,
                    height: o.source.height,
                    scale: None,
                },
                formats: o.formats,
                max_frame_rate: o.max_frame_rate,
                audio: o.audio,
                present: o.present,
            }),
            Self::Start(s) => ScreenMessage::Start(Start {
                display: s.camera,
                window: None,
                formats: s.formats,
                width: s.width,
                height: s.height,
                frame_rate: s.frame_rate,
                audio: s.audio,
            }),
            Self::Started(s) => ScreenMessage::Started(Started {
                display: Some(s.camera),
                window: None,
                format: s.format,
                audio: s.audio,
                width: s.width,
                height: s.height,
                frame_rate: s.frame_rate,
            }),
            Self::Refused(r) => ScreenMessage::Refused(r),
            Self::Frame(f) => ScreenMessage::Frame(f),
            Self::Audio(a) => ScreenMessage::Audio(a),
            Self::Keyframe => ScreenMessage::Keyframe,
            Self::Stop(s) => ScreenMessage::Stop(s),
        }
    }

    /// The screen family's message under the camera's names. A listing
    /// becomes the `cameras` request; an answer with its list is built from
    /// [`Cameras`] directly.
    #[must_use]
    pub fn from_screen(message: ScreenMessage) -> Self {
        match message {
            ScreenMessage::Displays(_) => Self::Cameras(Cameras::default()),
            ScreenMessage::Offer(o) => Self::Offer(CameraOffer {
                source: CameraSource {
                    kind: SourceKind::Camera,
                    id: o.source.id,
                    name: o.source.name,
                    width: o.source.width,
                    height: o.source.height,
                },
                formats: o.formats,
                max_frame_rate: o.max_frame_rate,
                audio: o.audio,
                present: o.present,
            }),
            ScreenMessage::Start(s) => Self::Start(CameraStart {
                camera: s.display,
                formats: s.formats,
                width: s.width,
                height: s.height,
                frame_rate: s.frame_rate,
                audio: s.audio,
            }),
            ScreenMessage::Started(s) => Self::Started(CameraStarted {
                camera: s.source_id().to_owned(),
                format: s.format,
                audio: s.audio,
                width: s.width,
                height: s.height,
                frame_rate: s.frame_rate,
            }),
            ScreenMessage::Refused(r) => Self::Refused(r),
            ScreenMessage::Frame(f) => Self::Frame(f),
            ScreenMessage::Audio(a) => Self::Audio(a),
            ScreenMessage::Keyframe => Self::Keyframe,
            ScreenMessage::Stop(s) => Self::Stop(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Scope;
    use crate::screen::{RefuseReason, StopReason};
    use crate::wire::Bytes;

    fn camera() -> Camera {
        Camera {
            id: "0".into(),
            name: Some("Back camera".into()),
            facing: Facing::Back,
            width: 4080,
            height: 3072,
            frame_rates: vec![30, 60],
        }
    }

    #[test]
    fn the_params_are_the_screen_family_s_with_presents_under_the_camera_s_name() {
        let params = ScreenParams {
            shares: true,
            views: true,
            receives: true,
            presents: true,
            audio_encodes: vec![AudioFormat::OPUS_MONO],
            ..ScreenParams::default()
        };
        let d = camera_descriptor(&params);
        assert_eq!(d.capability_type.as_str(), CAMERA_TYPE);
        assert_eq!(d.scope, Scope::Device);
        assert_eq!(camera_params(&d), Some(params.clone()));
        // A camera's descriptor is not a screen's, nor the other way round.
        assert_eq!(ScreenParams::from_descriptor(&d), None);
        assert_eq!(
            camera_params(&crate::screen::screen_descriptor(&params)),
            None
        );
    }

    #[test]
    fn messages_round_trip_under_the_camera_s_names() {
        let messages = [
            CameraMessage::Cameras(Cameras::default()),
            CameraMessage::Cameras(Cameras {
                cameras: Some(vec![camera()]),
            }),
            CameraMessage::Offer(CameraOffer {
                source: CameraSource {
                    kind: SourceKind::Camera,
                    id: "0".into(),
                    name: Some("Back camera".into()),
                    width: 4080,
                    height: 3072,
                },
                formats: vec![Format::H264],
                max_frame_rate: 60,
                audio: vec![AudioFormat::OPUS_MONO],
                present: true,
            }),
            CameraMessage::Start(CameraStart {
                camera: Some("0".into()),
                formats: vec![Format::H264],
                width: 1280,
                height: 720,
                frame_rate: 30,
                audio: vec![AudioFormat::OPUS_MONO],
            }),
            CameraMessage::Started(CameraStarted {
                camera: "0".into(),
                format: Format::H264,
                audio: Some(AudioFormat::OPUS_MONO),
                width: 1280,
                height: 720,
                frame_rate: 30,
            }),
            CameraMessage::Refused(Refused {
                reason: RefuseReason::Consent,
            }),
            CameraMessage::Frame(Frame {
                seq: 1,
                at: 33_333,
                key: true,
                last: true,
                bytes: Bytes(vec![0, 0, 0, 1, 0x65]),
            }),
            CameraMessage::Audio(Audio {
                seq: 2,
                at: 40_000,
                bytes: Bytes(vec![1, 2]),
            }),
            CameraMessage::Keyframe,
            CameraMessage::Stop(Stop {
                reason: StopReason::Viewer,
            }),
        ];
        for m in &messages {
            let mut bytes = Vec::new();
            ciborium::into_writer(m, &mut bytes).unwrap();
            let back: CameraMessage = ciborium::from_reader(bytes.as_slice()).unwrap();
            assert_eq!(&back, m);
        }
        let json = serde_json::to_value(&messages[1]).unwrap();
        assert_eq!(
            json["type"],
            "org.ligatureprotocol.capability.camera.cameras"
        );
        assert_eq!(json["cameras"][0]["facing"], "back");
        assert_eq!(json["cameras"][0]["frameRates"][1], 60);
        let json = serde_json::to_value(&messages[2]).unwrap();
        assert_eq!(json["source"]["kind"], "camera");
        assert_eq!(json["present"], true);
        let json = serde_json::to_value(&messages[3]).unwrap();
        assert_eq!(json["camera"], "0");
        assert!(json.get("display").is_none());
        let json = serde_json::to_value(&messages[4]).unwrap();
        assert_eq!(json["camera"], "0");
        assert_eq!(json["audio"]["channels"], 1);
        // A facing this device does not know is external, and one left out too.
        let unknown: Camera = serde_json::from_value(serde_json::json!({
            "id": "9", "width": 640, "height": 480, "facing": "periscope"
        }))
        .unwrap();
        assert_eq!(unknown.facing, Facing::External);
        let absent: Camera =
            serde_json::from_value(serde_json::json!({"id": "9", "width": 640, "height": 480}))
                .unwrap();
        assert_eq!(absent.facing, Facing::External);
        assert_eq!(Facing::Front.to_string(), "front");
    }

    #[test]
    fn a_camera_message_is_the_screen_family_s_by_another_name() {
        // start, started and offer keep everything but the names.
        let start = CameraMessage::Start(CameraStart {
            camera: Some("0".into()),
            formats: vec![Format::H264],
            width: 1280,
            height: 720,
            frame_rate: 30,
            audio: vec![AudioFormat::OPUS_MONO],
        });
        let ScreenMessage::Start(as_screen) = start.clone().into_screen() else {
            panic!("a start stays a start");
        };
        assert_eq!(as_screen.display.as_deref(), Some("0"));
        assert_eq!(as_screen.window, None);
        assert_eq!(
            CameraMessage::from_screen(ScreenMessage::Start(as_screen)),
            start
        );
        let started = CameraMessage::Started(CameraStarted {
            camera: "0".into(),
            format: Format::H264,
            audio: None,
            width: 640,
            height: 480,
            frame_rate: 15,
        });
        assert_eq!(
            CameraMessage::from_screen(started.clone().into_screen()),
            started
        );
        let offer = CameraMessage::Offer(CameraOffer {
            source: CameraSource {
                kind: SourceKind::Camera,
                id: "0".into(),
                name: None,
                width: 640,
                height: 480,
            },
            formats: vec![Format::H264],
            max_frame_rate: 30,
            audio: Vec::new(),
            present: true,
        });
        let ScreenMessage::Offer(as_screen) = offer.clone().into_screen() else {
            panic!("an offer stays an offer");
        };
        assert_eq!(as_screen.source.kind, SourceKind::Camera);
        assert!(as_screen.present);
        assert_eq!(
            CameraMessage::from_screen(ScreenMessage::Offer(as_screen)),
            offer
        );
        // The rest pass through as they are, and a listing is the request.
        for m in [
            CameraMessage::Keyframe,
            CameraMessage::Stop(Stop {
                reason: StopReason::Sharer,
            }),
            CameraMessage::Refused(Refused {
                reason: RefuseReason::Busy,
            }),
            CameraMessage::Cameras(Cameras::default()),
        ] {
            assert_eq!(CameraMessage::from_screen(m.clone().into_screen()), m);
        }
        assert_eq!(
            CameraMessage::Cameras(Cameras {
                cameras: Some(vec![camera()])
            })
            .into_screen(),
            ScreenMessage::Displays(Displays::default())
        );
    }
}
