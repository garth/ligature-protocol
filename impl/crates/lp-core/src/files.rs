//! The files capability: descriptor params, the file descriptor and the
//! messages of a transfer. Spec chapter 05 section 3d;
//! `schemas/capability.files.schema.json`.
//!
//! A transfer is a control stream carrying `request`, `offer`, `accept` and
//! `decline`, and one data stream per accepted file carrying `begin`, `chunk`
//! and `end` from the sender and `received` or `failed` from the receiver.
//! Every stream of a transfer names its `transfer` id.

use serde::{Deserialize, Serialize};

use crate::capability::{Descriptor, Scope};
use crate::wire::{Bytes, Timestamp};

/// Type name of the files capability, and the purpose of its streams.
pub const FILES_TYPE: &str = "org.ligatureprotocol.capability.files";

/// Application error code of a stream reset for a cancel the user asked for:
/// the family's first code (ADR 0038).
pub const CANCELLED_CODE: u32 = crate::frame::FAMILY_CODES_FROM;
/// Application error code of a stream reset for a failure (ADR 0038).
pub const FAILED_CODE: u32 = crate::frame::FAMILY_CODES_FROM + 1;

/// The files descriptor's `params`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilesParams {
    /// The device offers content and answers fetches.
    pub send: bool,
    /// The device accepts pushed files.
    pub receive: bool,
    /// The largest `chunk` payload, in bytes, the device accepts.
    pub max_chunk_bytes: u64,
    /// How many data streams the device accepts at once from one peer.
    pub max_streams: u32,
}

impl FilesParams {
    /// The default chunk bound: 256 KiB.
    pub const DEFAULT_MAX_CHUNK_BYTES: u64 = 262_144;
    /// The largest chunk the schema allows: the frame limit less 1 KiB of
    /// headroom for the rest of the message.
    pub const MAX_CHUNK_BYTES: u64 = 1024 * 1024 - 1024;

    /// Reads the params of a files descriptor; `None` if it is not one.
    /// Missing fields take the schema's defaults; a chunk bound over the
    /// schema's maximum is clamped to it.
    #[must_use]
    pub fn from_descriptor(descriptor: &Descriptor) -> Option<Self> {
        if descriptor.capability_type.as_str() != FILES_TYPE {
            return None;
        }
        let mut params = Self {
            send: true,
            receive: true,
            max_chunk_bytes: Self::DEFAULT_MAX_CHUNK_BYTES,
            max_streams: 1,
        };
        if let Some(ciborium::Value::Map(entries)) = &descriptor.params {
            for (k, v) in entries {
                match (k.as_text(), v) {
                    (Some("send"), ciborium::Value::Bool(b)) => params.send = *b,
                    (Some("receive"), ciborium::Value::Bool(b)) => params.receive = *b,
                    (Some("maxChunkBytes"), ciborium::Value::Integer(i)) => {
                        params.max_chunk_bytes = u64::try_from(*i)
                            .unwrap_or(Self::DEFAULT_MAX_CHUNK_BYTES)
                            .clamp(1, Self::MAX_CHUNK_BYTES);
                    }
                    (Some("maxStreams"), ciborium::Value::Integer(i)) => {
                        params.max_streams = u32::try_from(*i).unwrap_or(1).max(1);
                    }
                    _ => {}
                }
            }
        }
        Some(params)
    }
}

/// The descriptor a device advertises for files.
#[must_use]
pub fn files_descriptor(params: FilesParams) -> Descriptor {
    let mut d = Descriptor::new(FILES_TYPE, Scope::User);
    d.params = Some(ciborium::Value::Map(vec![
        (
            ciborium::Value::Text("send".into()),
            ciborium::Value::Bool(params.send),
        ),
        (
            ciborium::Value::Text("receive".into()),
            ciborium::Value::Bool(params.receive),
        ),
        (
            ciborium::Value::Text("maxChunkBytes".into()),
            ciborium::Value::Integer(params.max_chunk_bytes.into()),
        ),
        (
            ciborium::Value::Text("maxStreams".into()),
            ciborium::Value::Integer(params.max_streams.into()),
        ),
    ]));
    d
}

/// What a sender offers, described the same way whatever is behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDescriptor {
    /// Chosen by the sender, unique among what it offers.
    pub id: String,
    /// A base name, never a path.
    pub name: String,
    /// The exact size in bytes.
    pub size: u64,
    /// The content type; `application/octet-stream` when unknown.
    #[serde(default = "octet_stream")]
    pub mime: String,
    /// SHA-256 of the content, base64url unpadded, when known in advance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// When the content was last modified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<Timestamp>,
}

fn octet_stream() -> String {
    "application/octet-stream".to_owned()
}

impl FileDescriptor {
    /// The MIME type used when none is known.
    pub const OCTET_STREAM: &'static str = "application/octet-stream";

    /// A descriptor with the unknown content type and no hash or time.
    #[must_use]
    pub fn new(id: &str, name: &str, size: u64) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            size,
            mime: octet_stream(),
            sha256: None,
            modified: None,
        }
    }
}

/// Why a transfer was declined or a file failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    /// The sender no longer offers an id.
    Gone,
    /// The receiver does not take it.
    Refused,
    /// The receiver cannot hold a file this large.
    TooLarge,
    /// The receiver has no room.
    NoSpace,
    /// Too many transfers pending.
    Busy,
    /// What arrived does not match the descriptor's size.
    SizeMismatch,
    /// What arrived does not match the hash in `end`.
    HashMismatch,
    /// The receiver could not write it.
    WriteFailed,
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Gone => "gone",
            Self::Refused => "refused",
            Self::TooLarge => "too-large",
            Self::NoSpace => "no-space",
            Self::Busy => "busy",
            Self::SizeMismatch => "size-mismatch",
            Self::HashMismatch => "hash-mismatch",
            Self::WriteFailed => "write-failed",
        })
    }
}

/// `request`: on a control stream the receiver opened, the ids it wants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesRequest {
    /// The transfer id the receiver chose.
    pub transfer: String,
    /// The ids, announced earlier by the sender.
    pub ids: Vec<String>,
}

/// `offer`: on a control stream, the files the sender offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesOffer {
    /// The transfer id.
    pub transfer: String,
    /// The files.
    pub files: Vec<FileDescriptor>,
}

/// `accept`: the offered ids the receiver will take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesAccept {
    /// The ids, in the offer's order.
    pub ids: Vec<String>,
}

/// `decline`: the transfer does not go ahead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesDecline {
    /// Why.
    pub reason: Reason,
}

/// `begin`: the first message on a data stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesBegin {
    /// The transfer the file belongs to.
    pub transfer: String,
    /// The accepted file that follows.
    pub id: String,
}

/// `chunk`: the next bytes of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesChunk {
    /// The payload; a byte string in CBOR.
    pub bytes: Bytes,
}

/// `end`: the hash of everything sent on this data stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesEnd {
    /// SHA-256, base64url unpadded.
    pub sha256: String,
}

/// `failed`: the file did not check out or could not be written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesFailed {
    /// Why.
    pub reason: Reason,
}

/// One message of the files capability, on a control or a data stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FilesMessage {
    /// A fetch's first message.
    #[serde(rename = "org.ligatureprotocol.capability.files.request")]
    Request(FilesRequest),
    /// A push's first message, or the answer to a request.
    #[serde(rename = "org.ligatureprotocol.capability.files.offer")]
    Offer(FilesOffer),
    /// The receiver takes these.
    #[serde(rename = "org.ligatureprotocol.capability.files.accept")]
    Accept(FilesAccept),
    /// Nothing goes ahead.
    #[serde(rename = "org.ligatureprotocol.capability.files.decline")]
    Decline(FilesDecline),
    /// A data stream's first message.
    #[serde(rename = "org.ligatureprotocol.capability.files.begin")]
    Begin(FilesBegin),
    /// Bytes.
    #[serde(rename = "org.ligatureprotocol.capability.files.chunk")]
    Chunk(FilesChunk),
    /// The hash after the last chunk.
    #[serde(rename = "org.ligatureprotocol.capability.files.end")]
    End(FilesEnd),
    /// The file is in place.
    #[serde(rename = "org.ligatureprotocol.capability.files.received")]
    Received,
    /// The file was not kept.
    #[serde(rename = "org.ligatureprotocol.capability.files.failed")]
    Failed(FilesFailed),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_round_trip_through_the_descriptor_with_defaults_for_the_rest() {
        let params = FilesParams {
            send: true,
            receive: false,
            max_chunk_bytes: 4096,
            max_streams: 4,
        };
        let d = files_descriptor(params);
        assert_eq!(FilesParams::from_descriptor(&d), Some(params));
        let bare = Descriptor::new(FILES_TYPE, Scope::User);
        let defaults = FilesParams::from_descriptor(&bare).unwrap();
        assert!(defaults.send && defaults.receive);
        assert_eq!(
            defaults.max_chunk_bytes,
            FilesParams::DEFAULT_MAX_CHUNK_BYTES
        );
        assert_eq!(defaults.max_streams, 1);
        let other = Descriptor::new("org.example.other", Scope::User);
        assert!(FilesParams::from_descriptor(&other).is_none());
    }

    #[test]
    fn an_oversize_chunk_bound_is_clamped() {
        let mut d = Descriptor::new(FILES_TYPE, Scope::User);
        d.params = Some(ciborium::Value::Map(vec![(
            ciborium::Value::Text("maxChunkBytes".into()),
            ciborium::Value::Integer((8 * 1024 * 1024).into()),
        )]));
        assert_eq!(
            FilesParams::from_descriptor(&d).unwrap().max_chunk_bytes,
            FilesParams::MAX_CHUNK_BYTES
        );
    }

    #[test]
    fn messages_are_tagged_by_type_and_a_chunk_is_a_byte_string() {
        let chunk = FilesMessage::Chunk(FilesChunk {
            bytes: Bytes(vec![1, 2, 3]),
        });
        let mut cbor = Vec::new();
        ciborium::into_writer(&chunk, &mut cbor).unwrap();
        let back: FilesMessage = ciborium::from_reader(cbor.as_slice()).unwrap();
        assert_eq!(back, chunk);
        assert!(
            cbor.windows(4).any(|w| w == [0x43, 1, 2, 3]),
            "major type 2 byte string"
        );
        let received = FilesMessage::Received;
        let json = serde_json::to_value(&received).unwrap();
        assert_eq!(
            json["type"],
            "org.ligatureprotocol.capability.files.received"
        );
        let failed: FilesMessage = serde_json::from_value(serde_json::json!({
            "type": "org.ligatureprotocol.capability.files.failed",
            "reason": "hash-mismatch"
        }))
        .unwrap();
        assert_eq!(
            failed,
            FilesMessage::Failed(FilesFailed {
                reason: Reason::HashMismatch
            })
        );
    }
}
