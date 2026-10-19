//! Public keys as identifiers. ADR 0027 and ADR 0034.
//!
//! Every person, device and group is identified by an Ed25519 public key,
//! rendered as a multibase string: base58btc (`z` prefix) over the
//! `ed25519-pub` multicodec prefix followed by the 32 key bytes, which always
//! begins `z6Mk`. The string form is what schemas, JSON and CBOR carry; the
//! bytes are what cryptography uses. There is no other identifier.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Error;

/// The `ed25519-pub` multicodec code (0xed) as an unsigned varint.
pub const ED25519_PUB_PREFIX: [u8; 2] = [0xed, 0x01];

/// Length of an Ed25519 public key.
pub const PUBLIC_KEY_LEN: usize = 32;

/// An Ed25519 public key: the identifier of a person, device or group.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PublicKey([u8; PUBLIC_KEY_LEN]);

impl PublicKey {
    /// Wraps raw key bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; PUBLIC_KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// The raw key bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; PUBLIC_KEY_LEN] {
        &self.0
    }

    /// The multibase string (`z6Mk…`).
    #[must_use]
    pub fn to_multibase(&self) -> String {
        let mut bytes = Vec::with_capacity(ED25519_PUB_PREFIX.len() + PUBLIC_KEY_LEN);
        bytes.extend_from_slice(&ED25519_PUB_PREFIX);
        bytes.extend_from_slice(&self.0);
        multibase::encode(multibase::Base::Base58Btc, bytes)
    }

    /// Parses a multibase string, requiring base58btc and the Ed25519 prefix.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidKey`] for any other base, prefix or length.
    pub fn parse(s: &str) -> Result<Self, Error> {
        let (base, bytes) =
            multibase::decode(s).map_err(|e| Error::InvalidKey(format!("{s}: {e}")))?;
        if base != multibase::Base::Base58Btc {
            return Err(Error::InvalidKey(format!("{s}: not base58btc")));
        }
        let Some(key) = bytes.strip_prefix(&ED25519_PUB_PREFIX) else {
            return Err(Error::InvalidKey(format!("{s}: not an ed25519-pub key")));
        };
        let key: [u8; PUBLIC_KEY_LEN] = key
            .try_into()
            .map_err(|_| Error::InvalidKey(format!("{s}: wrong length")))?;
        Ok(Self(key))
    }

    /// A short form for terminals and logs: the eight characters after the
    /// constant `z6Mk` prefix. Presentation only; never stored or compared.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let s = self.to_multibase();
        s.get(4..12).unwrap_or(&s).to_owned()
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_multibase())
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.to_multibase())
    }
}

impl FromStr for PublicKey {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        Self::parse(s)
    }
}

impl Serialize for PublicKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_multibase())
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// Declares a newtype over [`PublicKey`] with the same string form.
macro_rules! key_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub PublicKey);

        impl $name {
            /// The underlying key.
            #[must_use]
            pub const fn key(&self) -> &PublicKey {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl FromStr for $name {
            type Err = Error;

            fn from_str(s: &str) -> Result<Self, Error> {
                PublicKey::parse(s).map(Self)
            }
        }

        impl From<PublicKey> for $name {
            fn from(key: PublicKey) -> Self {
                Self(key)
            }
        }
    };
}

key_newtype! {
    /// A person's master public key: the root of trust for everything the
    /// person owns or knows. Spec chapter 01.
    PersonKey
}

key_newtype! {
    /// A device's principal or binding public key. Spec chapter 02.
    DeviceKey
}

key_newtype! {
    /// A group's public key: signs the membership document and owns
    /// group-owned devices. Spec chapter 10.
    GroupKey
}

#[cfg(test)]
mod tests {
    use super::*;

    const BYTES: [u8; 32] = [7; 32];

    #[test]
    fn renders_with_the_z6mk_prefix_and_round_trips() {
        let key = PublicKey::from_bytes(BYTES);
        let s = key.to_multibase();
        assert!(s.starts_with("z6Mk"), "{s}");
        assert_eq!(PublicKey::parse(&s).unwrap(), key);
        assert_eq!(s.parse::<DeviceKey>().unwrap().key(), &key);
        assert_eq!(key.fingerprint().len(), 8);
        assert_eq!(key.to_string(), s);
    }

    #[test]
    fn rejects_other_bases_prefixes_and_lengths() {
        let key = PublicKey::from_bytes(BYTES);
        let mut raw = ED25519_PUB_PREFIX.to_vec();
        raw.extend_from_slice(&BYTES);
        let base32 = multibase::encode(multibase::Base::Base32Lower, &raw);
        assert!(PublicKey::parse(&base32).is_err(), "other base");
        let wrong_prefix = multibase::encode(multibase::Base::Base58Btc, [0xec, 0x01, 1, 2]);
        assert!(PublicKey::parse(&wrong_prefix).is_err(), "other prefix");
        let short = multibase::encode(multibase::Base::Base58Btc, &raw[..20]);
        assert!(PublicKey::parse(&short).is_err(), "short");
        assert!(PublicKey::parse("").is_err(), "empty");
        assert!(PublicKey::parse("not a key").is_err(), "garbage");
        let _ = key;
    }

    #[test]
    fn serde_uses_the_string_form() {
        let key = PersonKey(PublicKey::from_bytes(BYTES));
        let mut cbor = Vec::new();
        ciborium::into_writer(&key, &mut cbor).unwrap();
        let value: ciborium::Value = ciborium::from_reader(cbor.as_slice()).unwrap();
        assert_eq!(value, ciborium::Value::Text(key.to_string()));
        let back: PersonKey = ciborium::from_reader(cbor.as_slice()).unwrap();
        assert_eq!(back, key);
        let bad: Result<PersonKey, _> = ciborium::from_reader(b"\x63abc".as_slice());
        assert!(bad.is_err());
    }
}
