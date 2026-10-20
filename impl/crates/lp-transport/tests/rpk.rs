//! The ADR 0028 gate: mutual raw-public-key QUIC on loopback. Four points:
//! mutual authentication with anything else refused, the peer key readable as
//! a multibase string, the exporter, and connection migration.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use lp_crypto::session::{ALPN, RawKeyVerifier, SessionConfig};
use lp_crypto::signing::SigningKey;
use lp_transport::quic::Endpoint;
use rustls::client::danger::ServerCertVerifier;

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

#[tokio::test]
async fn both_sides_learn_the_other_key_and_the_exporter_agrees() {
    let a_key = SigningKey::generate().unwrap();
    let b_key = SigningKey::generate().unwrap();
    let a = Endpoint::bind(loopback(), &a_key).unwrap();
    let b = Endpoint::bind(loopback(), &b_key).unwrap();
    let b_addr = b.local_addr().unwrap();
    assert_ne!(b_addr.port(), 0, "the operating system assigned a port");

    let accept = tokio::spawn({
        let b = b.clone();
        async move { b.accept().await.unwrap().unwrap() }
    });
    let ab = a.connect(b_addr).await.unwrap();
    let ba = accept.await.unwrap();

    assert_eq!(ab.peer(), b_key.public());
    assert_eq!(ba.peer(), a_key.public());
    assert!(ab.peer().to_multibase().starts_with("z6Mk"));

    let label = b"EXPORTER-ligatureprotocol-pairing-sas";
    let x = ab.export_keying_material(label, b"ctx", 32).unwrap();
    let y = ba.export_keying_material(label, b"ctx", 32).unwrap();
    assert_eq!(x, y, "exporter output matches on both sides");
    let z = ab.export_keying_material(label, b"other", 32).unwrap();
    assert_ne!(x, z, "context changes the output");

    // Data flows on a stream.
    let (mut send, _recv) = ab.open_bi().await.unwrap();
    send.write_all(b"hello").await.unwrap();
    send.finish().unwrap();
    let (_s, mut recv) = ba.accept_bi().await.unwrap();
    let got = recv.read_to_end(64).await.unwrap();
    assert_eq!(got, b"hello");
}

#[tokio::test]
async fn a_connection_survives_rebinding_the_client_socket() {
    let a_key = SigningKey::generate().unwrap();
    let b_key = SigningKey::generate().unwrap();
    let a = Endpoint::bind(loopback(), &a_key).unwrap();
    let b = Endpoint::bind(loopback(), &b_key).unwrap();
    let b_addr = b.local_addr().unwrap();
    let accept = tokio::spawn({
        let b = b.clone();
        async move { b.accept().await.unwrap().unwrap() }
    });
    let ab = a.connect(b_addr).await.unwrap();
    let ba = accept.await.unwrap();
    let before = ba.remote_address();

    a.rebind(loopback()).unwrap();
    let after_local = a.local_addr().unwrap();
    assert_ne!(before.port(), after_local.port());

    let (mut send, _recv) = ab.open_bi().await.unwrap();
    send.write_all(b"after migration").await.unwrap();
    send.finish().unwrap();
    let (_s, mut recv) = ba.accept_bi().await.unwrap();
    assert_eq!(recv.read_to_end(64).await.unwrap(), b"after migration");
    assert_eq!(
        ba.remote_address().port(),
        after_local.port(),
        "server sees the new path"
    );
    assert_eq!(ba.peer(), a_key.public(), "same authenticated peer");
}

/// A client with no client certificate at all is refused: client auth is
/// mandatory and only raw public keys are negotiated.
#[tokio::test]
async fn a_client_without_a_key_is_refused() {
    let b_key = SigningKey::generate().unwrap();
    let b = Endpoint::bind(loopback(), &b_key).unwrap();
    let b_addr = b.local_addr().unwrap();
    tokio::spawn({
        let b = b.clone();
        async move {
            let r = b.accept().await.unwrap();
            assert!(r.is_err(), "server must not complete the handshake");
        }
    });

    let mut client = rustls::ClientConfig::builder_with_provider(lp_crypto::session::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(RawKeyVerifier::new())
        .with_no_client_auth();
    client.alpn_protocols = vec![ALPN.to_vec()];
    let result = dial_with(client, b_addr).await;
    assert!(
        result.is_err(),
        "client without a key must be refused: {result:?}"
    );
}

/// A client that would accept an X.509 server certificate (does not require
/// raw public keys) cannot negotiate with a server that only has one.
#[tokio::test]
async fn a_client_not_requiring_raw_public_keys_is_refused() {
    #[derive(Debug)]
    struct X509Only;
    impl ServerCertVerifier for X509Only {
        fn verify_server_cert(
            &self,
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            vec![rustls::SignatureScheme::ED25519]
        }
    }

    let a_key = SigningKey::generate().unwrap();
    let b_key = SigningKey::generate().unwrap();
    let b = Endpoint::bind(loopback(), &b_key).unwrap();
    let b_addr = b.local_addr().unwrap();
    tokio::spawn({
        let b = b.clone();
        async move {
            let r = b.accept().await.unwrap();
            assert!(r.is_err(), "server must not complete the handshake");
        }
    });
    let session = SessionConfig::new(&a_key).unwrap();
    let provider = lp_crypto::session::provider();
    let certified = Arc::new(lp_crypto::session::certified_key(&a_key, &provider).unwrap());
    let mut client = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(X509Only))
        .with_client_cert_resolver(Arc::new(
            rustls::client::AlwaysResolvesClientRawPublicKeys::new(certified),
        ));
    client.alpn_protocols = vec![ALPN.to_vec()];
    let _ = session;
    let result = dial_with(client, b_addr).await;
    assert!(
        result.is_err(),
        "x509-accepting client must be refused: {result:?}"
    );
}

async fn dial_with(client: rustls::ClientConfig, addr: SocketAddr) -> Result<(), String> {
    let crypto =
        quinn::crypto::rustls::QuicClientConfig::try_from(client).map_err(|e| e.to_string())?;
    let mut ep = quinn::Endpoint::client(loopback()).map_err(|e| e.to_string())?;
    ep.set_default_client_config(quinn::ClientConfig::new(Arc::new(crypto)));
    let conn = ep
        .connect(addr, "ligature")
        .map_err(|e| e.to_string())?
        .await
        .map_err(|e| e.to_string())?;
    drop(conn);
    Ok(())
}

/// An idle connection stays up with keep-alives and dies without them.
#[tokio::test]
async fn keep_alives_hold_an_idle_connection_open() {
    use lp_transport::quic::Timeouts;
    let a_key = SigningKey::generate().unwrap();
    let b_key = SigningKey::generate().unwrap();
    let short = Timeouts {
        keep_alive: Some(Duration::from_millis(200)),
        idle: Duration::from_millis(1500),
    };
    let a = Endpoint::bind_with(loopback(), &a_key, short).unwrap();
    let b = Endpoint::bind_with(loopback(), &b_key, short).unwrap();
    let accept = tokio::spawn({
        let b = b.clone();
        async move { b.accept().await.unwrap().unwrap() }
    });
    let ab = a.connect(b.local_addr().unwrap()).await.unwrap();
    let ba = accept.await.unwrap();
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert!(
        ab.raw().close_reason().is_none(),
        "kept alive: {:?}",
        ab.raw().close_reason()
    );
    assert!(ba.raw().close_reason().is_none());

    let none = Timeouts {
        keep_alive: None,
        idle: Duration::from_millis(1000),
    };
    let c = Endpoint::bind_with(loopback(), &SigningKey::generate().unwrap(), none).unwrap();
    let d = Endpoint::bind_with(loopback(), &SigningKey::generate().unwrap(), none).unwrap();
    let accept = tokio::spawn({
        let d = d.clone();
        async move { d.accept().await.unwrap().unwrap() }
    });
    let cd = c.connect(d.local_addr().unwrap()).await.unwrap();
    let _dc = accept.await.unwrap();
    tokio::time::sleep(Duration::from_millis(3000)).await;
    assert!(
        cd.raw().close_reason().is_some(),
        "idled out without keep-alives"
    );
}
