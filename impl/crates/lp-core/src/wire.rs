//! Scalar wire types shared by every record: bytes, timestamps and type names.
//! ADR 0020 and ADR 0025.
//!
//! Bytes are a CBOR byte string on the wire and base64url without padding in
//! JSON; timestamps are RFC 3339 strings everywhere; type names are
//! reverse-DNS strings such as `org.ligatureprotocol.person`.

use core::fmt;
use core::str::FromStr;

use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::Error;

/// Bytes: a CBOR byte string on the wire, base64url (no padding) in JSON.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct Bytes(pub Vec<u8>);

impl Bytes {
    /// The base64url form without padding.
    #[must_use]
    pub fn to_base64url(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&self.0)
    }

    /// Parses the base64url form.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if the text is not base64url.
    pub fn from_base64url(s: &str) -> Result<Self, Error> {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s)
            .map(Self)
            .map_err(|e| Error::Decode(format!("base64url: {e}")))
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

impl From<&[u8]> for Bytes {
    fn from(v: &[u8]) -> Self {
        Self(v.to_vec())
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({} bytes)", self.0.len())
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            s.serialize_str(&self.to_base64url())
        } else {
            s.serialize_bytes(&self.0)
        }
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Bytes;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a byte string or base64url text")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Bytes, E> {
                Ok(Bytes(v.to_vec()))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Bytes, E> {
                Ok(Bytes(v))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Bytes, E> {
                Bytes::from_base64url(v).map_err(E::custom)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Bytes, A::Error> {
                let mut out = Vec::new();
                while let Some(b) = seq.next_element::<u8>()? {
                    out.push(b);
                }
                Ok(Bytes(out))
            }
        }
        d.deserialize_any(V)
    }
}

/// An RFC 3339 timestamp with second precision, always UTC on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Timestamp(OffsetDateTime);

impl Timestamp {
    /// The current time.
    #[must_use]
    pub fn now() -> Self {
        Self(
            OffsetDateTime::now_utc()
                .replace_nanosecond(0)
                .unwrap_or_else(|_| OffsetDateTime::now_utc()),
        )
    }

    /// From seconds since the Unix epoch.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if out of range.
    pub fn from_unix(seconds: i64) -> Result<Self, Error> {
        OffsetDateTime::from_unix_timestamp(seconds)
            .map(Self)
            .map_err(|e| Error::Decode(format!("timestamp: {e}")))
    }

    /// Seconds since the Unix epoch.
    #[must_use]
    pub fn unix(&self) -> i64 {
        self.0.unix_timestamp()
    }

    /// The RFC 3339 form.
    #[must_use]
    pub fn to_rfc3339(&self) -> String {
        self.0
            .to_offset(time::UtcOffset::UTC)
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({})", self.to_rfc3339())
    }
}

impl FromStr for Timestamp {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        OffsetDateTime::parse(s, &Rfc3339)
            .map(Self)
            .map_err(|e| Error::Decode(format!("timestamp {s:?}: {e}")))
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_rfc3339())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// A reverse-DNS type name naming a schema.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TypeName(pub String);

impl TypeName {
    /// Builds a type name from a static string.
    #[must_use]
    pub fn new(s: &str) -> Self {
        Self(s.to_owned())
    }

    /// The string form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TypeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for TypeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TypeName({})", self.0)
    }
}

impl From<&str> for TypeName {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_a_byte_string_in_cbor_and_base64url_in_json() {
        let b = Bytes(vec![0, 255, 7]);
        let mut cbor = Vec::new();
        ciborium::into_writer(&b, &mut cbor).unwrap();
        assert_eq!(cbor, vec![0x43, 0, 255, 7], "major type 2, length 3");
        let back: Bytes = ciborium::from_reader(cbor.as_slice()).unwrap();
        assert_eq!(back, b);
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(json, "\"AP8H\"");
        let back: Bytes = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn timestamps_are_rfc3339_utc_strings() {
        let t = Timestamp::from_unix(1_760_000_000).unwrap();
        assert_eq!(t.to_rfc3339(), "2025-10-09T08:53:20Z");
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"2025-10-09T08:53:20Z\"");
        let back: Timestamp = serde_json::from_str("\"2025-10-09T10:53:20+02:00\"").unwrap();
        assert_eq!(back, t, "offsets are normalised");
        assert!("yesterday".parse::<Timestamp>().is_err());
        let mut cbor = Vec::new();
        ciborium::into_writer(&t, &mut cbor).unwrap();
        assert_eq!(cbor[0], 0x74, "text string of 20 characters");
    }
}
