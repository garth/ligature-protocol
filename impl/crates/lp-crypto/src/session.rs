//! Raw-public-key TLS material for QUIC sessions. Spec chapter 03; ADR 0028.
//!
//! A device presents its Ed25519 key as a TLS raw public key (RFC 7250): the
//! "certificate" on the wire is the key's `SubjectPublicKeyInfo`, nothing
//! more. Both sides require raw public keys, so an X.509 certificate can never
//! be negotiated. TLS 1.3 only, the rustls default suites with the `ring`
//! provider, mutual authentication, no resumption and therefore no 0-RTT,
//! ALPN `ligature/1` (ADR 0036).
//!
//! The verifier accepts any well-formed Ed25519 raw public key and leaves the
//! trust decision to the caller: `lp-transport` reads the peer key from the
//! finished handshake and `lp-identity` decides whether it is a binding of
//! mine, a principal I use, a known person's binding, or a stranger who gets
//! nothing but the pairing offer.

use std::fmt;
use std::sync::Arc;

use lp_core::{Error, PublicKey};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::sign::CertifiedKey;
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

use crate::signing::{Signature, SigningKey, verify};

/// The ALPN token: the protocol version (ADR 0036).
pub const ALPN: &[u8] = b"ligature/1";

/// DER prefix of an Ed25519 `SubjectPublicKeyInfo` (RFC 8410), followed by
/// the 32 key bytes.
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// DER prefix of an Ed25519 PKCS#8 v1 `PrivateKeyInfo` (RFC 8410), followed
/// by the 32 seed bytes.
const PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

/// The `SubjectPublicKeyInfo` DER of an Ed25519 key: what travels as the raw
/// public key "certificate".
#[must_use]
pub fn spki_der(key: &PublicKey) -> Vec<u8> {
    let mut der = SPKI_PREFIX.to_vec();
    der.extend_from_slice(key.as_bytes());
    der
}

/// Reads an Ed25519 key out of a `SubjectPublicKeyInfo` DER.
///
/// # Errors
///
/// Returns [`Error::InvalidKey`] if the DER is not exactly an Ed25519 SPKI.
pub fn public_key_from_spki(der: &[u8]) -> Result<PublicKey, Error> {
    let Some(key) = der.strip_prefix(&SPKI_PREFIX) else {
        return Err(Error::InvalidKey(
            "not an Ed25519 SubjectPublicKeyInfo".to_owned(),
        ));
    };
    let key: [u8; 32] = key
        .try_into()
        .map_err(|_| Error::InvalidKey("SubjectPublicKeyInfo has the wrong length".to_owned()))?;
    Ok(PublicKey::from_bytes(key))
}

fn pkcs8_der(key: &SigningKey) -> Vec<u8> {
    let mut der = PKCS8_PREFIX.to_vec();
    der.extend_from_slice(&key.seed());
    der
}

/// The crypto provider every session uses: rustls defaults over `ring`.
#[must_use]
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The device key as rustls signing material, with its SPKI as the one
/// "certificate".
///
/// # Errors
///
/// Returns [`Error::InvalidKey`] if the provider refuses the key.
pub fn certified_key(key: &SigningKey, provider: &CryptoProvider) -> Result<CertifiedKey, Error> {
    let der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8_der(key)));
    let signing = provider
        .key_provider
        .load_private_key(der)
        .map_err(|e| Error::InvalidKey(format!("rustls refused the device key: {e}")))?;
    let spki = CertificateDer::from(spki_der(key.public()));
    Ok(CertifiedKey::new(vec![spki], signing))
}

/// Accepts any well-formed Ed25519 raw public key, as server or client
/// verifier, and verifies the handshake signature with it. Trust is decided
/// after the handshake by whoever reads the peer key.
#[derive(Debug)]
pub struct RawKeyVerifier {
    roots: Vec<DistinguishedName>,
}

impl RawKeyVerifier {
    /// A verifier.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self { roots: Vec::new() })
    }

    fn check_spki(der: &CertificateDer<'_>) -> Result<PublicKey, rustls::Error> {
        public_key_from_spki(der.as_ref()).map_err(|e| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
                Arc::new(e),
            )))
        })
    }

    fn check_signature(
        message: &[u8],
        spki: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        if dss.scheme != SignatureScheme::ED25519 {
            return Err(rustls::Error::PeerIncompatible(
                rustls::PeerIncompatible::NoSignatureSchemesInCommon,
            ));
        }
        let key = Self::check_spki(spki)?;
        let sig = Signature::from_slice(dss.signature()).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadSignature)
        })?;
        verify(&key, message, &sig).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadSignature)
        })?;
        Ok(HandshakeSignatureValid::assertion())
    }
}

impl ServerCertVerifier for RawKeyVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if !intermediates.is_empty() {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        }
        Self::check_spki(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Self::check_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

impl ClientCertVerifier for RawKeyVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &self.roots
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        if !intermediates.is_empty() {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        }
        Self::check_spki(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Self::check_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }
}

/// TLS configuration for one device's QUIC endpoint: the same key presented
/// as server and as client.
#[derive(Clone)]
pub struct SessionConfig {
    /// For accepting connections.
    pub server: Arc<rustls::ServerConfig>,
    /// For dialling.
    pub client: Arc<rustls::ClientConfig>,
    /// The key both configurations present.
    pub public: PublicKey,
}

impl fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionConfig({})", self.public)
    }
}

impl SessionConfig {
    /// Builds both configurations for `key`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidKey`] if rustls refuses the key or the
    /// configuration.
    pub fn new(key: &SigningKey) -> Result<Self, Error> {
        let provider = provider();
        let certified = Arc::new(certified_key(key, &provider)?);
        let verifier = RawKeyVerifier::new();
        let cfg = |e: rustls::Error| Error::InvalidKey(format!("tls configuration: {e}"));

        let mut server = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(cfg)?
            .with_client_cert_verifier(verifier.clone())
            .with_cert_resolver(Arc::new(
                rustls::server::AlwaysResolvesServerRawPublicKeys::new(certified.clone()),
            ));
        server.alpn_protocols = vec![ALPN.to_vec()];
        server.send_tls13_tickets = 0;
        server.max_early_data_size = 0;

        let mut client = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(cfg)?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_cert_resolver(Arc::new(
                rustls::client::AlwaysResolvesClientRawPublicKeys::new(certified),
            ));
        client.alpn_protocols = vec![ALPN.to_vec()];
        client.resumption = rustls::client::Resumption::disabled();
        client.enable_early_data = false;

        Ok(Self {
            server: Arc::new(server),
            client: Arc::new(client),
            public: *key.public(),
        })
    }
}

/// Reads the peer's key from the certificate list a finished handshake
/// produced: exactly one raw public key.
///
/// # Errors
///
/// Returns [`Error::InvalidKey`] unless the list is one Ed25519 SPKI.
pub fn peer_key(certs: &[CertificateDer<'_>]) -> Result<PublicKey, Error> {
    match certs {
        [one] => public_key_from_spki(one.as_ref()),
        _ => Err(Error::InvalidKey(format!(
            "expected one raw public key, got {} certificates",
            certs.len()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spki_round_trips() {
        let key = SigningKey::generate().unwrap();
        let der = spki_der(key.public());
        assert_eq!(der.len(), 44);
        assert_eq!(public_key_from_spki(&der).unwrap(), *key.public());
        assert!(public_key_from_spki(&der[1..]).is_err());
        assert!(public_key_from_spki(&[]).is_err());
    }

    #[test]
    fn the_provider_loads_the_key_and_renders_its_spki() {
        let key = SigningKey::generate().unwrap();
        let ck = certified_key(&key, &provider()).unwrap();
        assert_eq!(ck.cert.len(), 1);
        assert_eq!(
            ck.key
                .public_key()
                .expect("ring knows the public key")
                .as_ref(),
            spki_der(key.public()).as_slice(),
            "ring's view of the SPKI matches ours"
        );
        let config = SessionConfig::new(&key).unwrap();
        assert_eq!(config.public, *key.public());
    }
}
