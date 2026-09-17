//! Certificates of the ACME TLS-ALPN-01 challenge (RFC 8737).

use rcgen::{CertificateParams, CustomExtension, KeyPair};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::crypto::ring::sign;
use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::server::{ClientHello, ResolvesServerCert};
use tokio_rustls::rustls::sign::CertifiedKey;

/// The ALPN protocol of the TLS-ALPN-01 challenge.
pub const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";

/// Returns true when the client offers `acme-tls/1` as its only ALPN protocol. RFC 8737 section 3
/// requires the validation server to offer only this protocol.
pub fn offers_only_acme_tls(hello: &ClientHello) -> bool {
    hello.alpn().is_some_and(only_acme_tls)
}

fn only_acme_tls<'a>(mut protocols: impl Iterator<Item = &'a [u8]>) -> bool {
    protocols.next() == Some(ACME_TLS_ALPN) && protocols.next().is_none()
}

/// A pending TLS-ALPN-01 challenge: the SHA-256 digest of the key authorization of a domain.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TlsAlpnChallenge {
    pub domain: String,
    pub digest: [u8; 32],
}

/// The challenge certificates, selected by the server name of the ClientHello.
#[derive(Debug, Default)]
pub struct ChallengeCerts {
    certs: HashMap<String, Arc<CertifiedKey>>,
}

impl ChallengeCerts {
    /// Builds a self-signed certificate for every challenge. Each node builds its own
    /// certificates, so the private keys never leave the process.
    pub fn new(challenges: &[TlsAlpnChallenge]) -> anyhow::Result<Self> {
        let certs = challenges
            .iter()
            .map(|challenge| {
                let cert = challenge_cert(challenge)?;
                Ok((challenge.domain.to_ascii_lowercase(), Arc::new(cert)))
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(Self { certs })
    }

    pub fn is_empty(&self) -> bool {
        self.certs.is_empty()
    }
}

impl ResolvesServerCert for ChallengeCerts {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        let name = client_hello.server_name()?.to_ascii_lowercase();
        self.certs.get(&name).cloned()
    }
}

/// Returns the TLS config that answers the challenge handshakes.
pub fn challenge_config(certs: Arc<ChallengeCerts>) -> Arc<ServerConfig> {
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(certs);
    config.alpn_protocols = vec![ACME_TLS_ALPN.to_vec()];
    Arc::new(config)
}

/// Builds the certificate with the domain as its only name and the critical acmeIdentifier
/// extension that holds the digest.
fn challenge_cert(challenge: &TlsAlpnChallenge) -> anyhow::Result<CertifiedKey> {
    let mut params = CertificateParams::new(vec![challenge.domain.clone()])?;
    params
        .custom_extensions
        .push(CustomExtension::new_acme_identifier(&challenge.digest));
    let keypair = KeyPair::generate()?;
    let cert = params.self_signed(&keypair)?;
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(keypair.serialize_der()));
    let signing_key = sign::any_supported_type(&key)?;
    Ok(CertifiedKey::new(vec![cert.der().clone()], signing_key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_rustls::rustls::server::Acceptor;
    use tokio_rustls::rustls::{ClientConfig, ClientConnection, RootCertStore};
    use x509_parser::extensions::GeneralName;
    use x509_parser::parse_x509_certificate;

    const ACME_IDENTIFIER_OID: &str = "1.3.6.1.5.5.7.1.31";

    fn challenge(domain: &str, byte: u8) -> TlsAlpnChallenge {
        TlsAlpnChallenge {
            domain: domain.to_string(),
            digest: [byte; 32],
        }
    }

    /// Writes the ClientHello of a client with the server name and the ALPN protocols, and
    /// passes the parsed ClientHello to `check`.
    fn with_client_hello(server_name: &str, alpn: &[&[u8]], check: impl FnOnce(ClientHello)) {
        let mut config = ClientConfig::builder()
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        let name = server_name.to_string().try_into().unwrap();
        let mut client = ClientConnection::new(Arc::new(config), name).unwrap();
        let mut hello = Vec::new();
        client.write_tls(&mut hello).unwrap();

        let mut acceptor = Acceptor::default();
        acceptor.read_tls(&mut hello.as_slice()).unwrap();
        let accepted = acceptor.accept().ok().flatten().unwrap();
        check(accepted.client_hello());
    }

    #[test]
    fn only_a_single_acme_tls_protocol_is_a_challenge() {
        let cases: [(&[&[u8]], bool); 4] = [
            (&[ACME_TLS_ALPN], true),
            (&[b"h2", ACME_TLS_ALPN], false),
            (&[ACME_TLS_ALPN, b"http/1.1"], false),
            (&[b"h2"], false),
        ];
        for (protocols, expected) in cases {
            assert_eq!(only_acme_tls(protocols.iter().copied()), expected);
            with_client_hello("example.com", protocols, |hello| {
                assert_eq!(offers_only_acme_tls(&hello), expected, "{protocols:?}");
            });
        }
        with_client_hello("example.com", &[], |hello| {
            assert!(!offers_only_acme_tls(&hello));
        });
    }

    #[test]
    fn the_certificate_holds_the_domain_and_a_critical_digest() {
        let cert = challenge_cert(&challenge("example.com", 7)).unwrap();
        let (_, x509) = parse_x509_certificate(cert.cert[0].as_ref()).unwrap();

        let names = x509
            .subject_alternative_name()
            .unwrap()
            .unwrap()
            .value
            .general_names
            .clone();
        assert_eq!(names, vec![GeneralName::DNSName("example.com")]);

        let extension = x509
            .extensions()
            .iter()
            .find(|ext| ext.oid.to_id_string() == ACME_IDENTIFIER_OID)
            .unwrap();
        assert!(extension.critical);
        let mut expected = vec![0x04, 32];
        expected.extend_from_slice(&[7; 32]);
        assert_eq!(extension.value, expected.as_slice());
    }

    #[test]
    fn the_resolver_selects_the_certificate_of_the_server_name() {
        let certs =
            ChallengeCerts::new(&[challenge("A.example.com", 1), challenge("b.example.com", 2)])
                .unwrap();
        assert!(!certs.is_empty());
        assert!(ChallengeCerts::default().is_empty());

        with_client_hello("a.example.com", &[ACME_TLS_ALPN], |hello| {
            let cert = certs.resolve(hello).unwrap();
            let (_, x509) = parse_x509_certificate(cert.cert[0].as_ref()).unwrap();
            let names = x509.subject_alternative_name().unwrap().unwrap();
            assert_eq!(
                names.value.general_names,
                vec![GeneralName::DNSName("A.example.com")]
            );
        });
        with_client_hello("c.example.com", &[ACME_TLS_ALPN], |hello| {
            assert!(certs.resolve(hello).is_none());
        });
    }

    #[test]
    fn the_challenge_config_offers_only_acme_tls() {
        let config = challenge_config(Arc::new(ChallengeCerts::default()));
        assert_eq!(config.alpn_protocols, vec![ACME_TLS_ALPN.to_vec()]);
    }
}
