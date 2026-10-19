//! Frames and typed messages on a stream. ADR 0036; `schemas/message.schema.json`.
//!
//! A frame is a 4-byte big-endian length followed by exactly one CBOR map.
//! Every map carries a `type` with the message's reverse-DNS schema name. The
//! first frame on every stream is the stream header naming its purpose. The
//! same codec serves QUIC streams and the local control socket.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::envelope::{Envelope, FactId};
use crate::wire::TypeName;

/// Default maximum message size, unless a capability says otherwise.
pub const MAX_FRAME: usize = 1024 * 1024;

/// Type name of the stream header.
pub const STREAM_TYPE: &str = "org.ligatureprotocol.stream";
/// Type name of `gossip.have`.
pub const GOSSIP_HAVE_TYPE: &str = "org.ligatureprotocol.gossip.have";
/// Type name of `gossip.want`.
pub const GOSSIP_WANT_TYPE: &str = "org.ligatureprotocol.gossip.want";
/// Type name of `gossip.fact`.
pub const GOSSIP_FACT_TYPE: &str = "org.ligatureprotocol.gossip.fact";

/// Application error code a receiver closes a stream with when it does not
/// understand the stream's purpose.
pub const UNKNOWN_PURPOSE_CODE: u32 = 1;

/// Encodes one message as a frame: length prefix plus CBOR.
///
/// # Errors
///
/// Returns [`Error::Decode`] if encoding fails or the message exceeds `max`.
pub fn encode<T: Serialize>(message: &T, max: usize) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    ciborium::into_writer(message, &mut body).map_err(|e| Error::Decode(format!("encode: {e}")))?;
    if body.len() > max {
        return Err(Error::Decode(format!(
            "message of {} bytes exceeds {max}",
            body.len()
        )));
    }
    let len =
        u32::try_from(body.len()).map_err(|_| Error::Decode("message too long".to_owned()))?;
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Reads the length prefix and says how many body bytes follow.
///
/// # Errors
///
/// Returns [`Error::Decode`] if the announced length exceeds `max`.
pub fn body_len(prefix: [u8; 4], max: usize) -> Result<usize, Error> {
    let len = u32::from_be_bytes(prefix) as usize;
    if len > max {
        return Err(Error::Decode(format!("frame of {len} bytes exceeds {max}")));
    }
    Ok(len)
}

/// Decodes one frame body. Forgiving: unknown fields are ignored.
///
/// # Errors
///
/// Returns [`Error::Decode`] if the body is not valid CBOR for `T`.
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    ciborium::from_reader(body).map_err(|e| Error::Decode(format!("decode: {e}")))
}

/// The `type` of a frame body, without decoding the rest.
///
/// # Errors
///
/// Returns [`Error::Decode`] if the body is not a map with a text `type`.
pub fn peek_type(body: &[u8]) -> Result<TypeName, Error> {
    #[derive(Deserialize)]
    struct Typed {
        #[serde(rename = "type")]
        fact_type: TypeName,
    }
    decode::<Typed>(body).map(|t| t.fact_type)
}

/// What a stream is for: the first frame on every stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamHeader {
    /// Always [`STREAM_TYPE`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// `gossip`, `manifest`, or a capability type.
    pub purpose: String,
}

impl StreamHeader {
    /// A header for `purpose`.
    #[must_use]
    pub fn new(purpose: &str) -> Self {
        Self {
            message_type: TypeName::new(STREAM_TYPE),
            purpose: purpose.to_owned(),
        }
    }

    /// The purpose of the gossip stream.
    pub const GOSSIP: &'static str = "gossip";
    /// The purpose of the manifest stream.
    pub const MANIFEST: &'static str = "manifest";
}

/// `gossip.have`: the fact identifiers this side holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GossipHave {
    /// Always [`GOSSIP_HAVE_TYPE`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// The identifiers.
    pub facts: Vec<FactId>,
}

impl GossipHave {
    /// A `have` message.
    #[must_use]
    pub fn new(facts: Vec<FactId>) -> Self {
        Self {
            message_type: TypeName::new(GOSSIP_HAVE_TYPE),
            facts,
        }
    }
}

/// `gossip.want`: the fact identifiers this side lacks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GossipWant {
    /// Always [`GOSSIP_WANT_TYPE`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// The identifiers.
    pub facts: Vec<FactId>,
}

impl GossipWant {
    /// A `want` message.
    #[must_use]
    pub fn new(facts: Vec<FactId>) -> Self {
        Self {
            message_type: TypeName::new(GOSSIP_WANT_TYPE),
            facts,
        }
    }
}

/// `gossip.fact`: one signed fact. Envelopes never travel bare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GossipFact {
    /// Always [`GOSSIP_FACT_TYPE`].
    #[serde(rename = "type")]
    pub message_type: TypeName,
    /// The fact, byte-exact.
    pub envelope: Envelope,
}

impl GossipFact {
    /// A `fact` message.
    #[must_use]
    pub fn new(envelope: Envelope) -> Self {
        Self {
            message_type: TypeName::new(GOSSIP_FACT_TYPE),
            envelope,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_carry_a_type() {
        let header = StreamHeader::new("gossip");
        let frame = encode(&header, MAX_FRAME).unwrap();
        let len = body_len(frame[..4].try_into().unwrap(), MAX_FRAME).unwrap();
        assert_eq!(len, frame.len() - 4);
        assert_eq!(peek_type(&frame[4..]).unwrap().as_str(), STREAM_TYPE);
        let back: StreamHeader = decode(&frame[4..]).unwrap();
        assert_eq!(back, header);
    }

    #[test]
    fn oversize_frames_are_refused_before_decoding() {
        assert!(body_len(u32::MAX.to_be_bytes(), MAX_FRAME).is_err());
        assert!(body_len(((MAX_FRAME + 1) as u32).to_be_bytes(), MAX_FRAME).is_err());
        assert!(body_len((MAX_FRAME as u32).to_be_bytes(), MAX_FRAME).is_ok());
        let big = GossipHave::new(vec![FactId([0; 32]); 10]);
        assert!(encode(&big, 16).is_err(), "encoder enforces the limit too");
    }

    #[test]
    fn truncated_bodies_fail_to_decode() {
        let frame = encode(&StreamHeader::new("manifest"), MAX_FRAME).unwrap();
        let body = &frame[4..];
        assert!(decode::<StreamHeader>(&body[..body.len() - 3]).is_err());
        assert!(peek_type(b"\x01").is_err(), "not a map");
    }

    #[test]
    fn unknown_fields_in_a_header_are_ignored() {
        let value = ciborium::Value::Map(vec![
            (
                ciborium::Value::Text("type".into()),
                ciborium::Value::Text(STREAM_TYPE.into()),
            ),
            (
                ciborium::Value::Text("purpose".into()),
                ciborium::Value::Text("gossip".into()),
            ),
            (
                ciborium::Value::Text("future".into()),
                ciborium::Value::Integer(7.into()),
            ),
        ]);
        let mut body = Vec::new();
        ciborium::into_writer(&value, &mut body).unwrap();
        let h: StreamHeader = decode(&body).unwrap();
        assert_eq!(h.purpose, "gossip");
    }
}
