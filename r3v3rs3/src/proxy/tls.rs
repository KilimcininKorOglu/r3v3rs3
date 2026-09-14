use crate::certs::alpn::offers_only_acme_tls;
use crate::certs::Cert;
use crate::server::cert_list::CertList;
use dashmap::DashMap;
use r3v3rs3_api::cert::CertKind;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::subject_name::SubjectName;
use r3v3rs3_api::tls::{ClientAuthMode, TlsState};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io;
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_rustls::rustls::pki_types::CertificateDer;
use tokio_rustls::rustls::server::danger::ClientCertVerifier;
use tokio_rustls::rustls::server::{
    Acceptor, ClientHello, ResolvesServerCert, WebPkiClientVerifier,
};
use tokio_rustls::rustls::sign::CertifiedKey;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::{server::TlsStream, LazyConfigAcceptor, TlsAcceptor};
use tracing::{debug, error};
use x509_parser::parse_x509_certificate;

pub struct TlsTermination {
    pub server_names: Vec<SubjectName>,
    pub acceptor: Option<TlsAcceptor>,
    pub alpn_protocols: Vec<Vec<u8>>,
    client_auth: ClientAuthMode,
    client_ca_certs: Vec<ShortId>,
}

impl fmt::Debug for TlsTermination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsTermination")
            .field("server_names", &self.server_names)
            .field("client_auth", &self.client_auth)
            .field("client_ca_certs", &self.client_ca_certs)
            .finish()
    }
}

impl TlsTermination {
    pub fn new(
        config: &r3v3rs3_api::tls::TlsTermination,
        alpn_protocols: Vec<Vec<u8>>,
    ) -> Result<Self, Error> {
        let mut server_names = Vec::new();
        for name in &config.server_names {
            let name = SubjectName::from_str(name)?;
            server_names.push(name);
        }
        Ok(Self {
            server_names,
            acceptor: None,
            alpn_protocols,
            client_auth: config.client_auth,
            client_ca_certs: config.client_ca_certs.clone(),
        })
    }

    /// Builds the TLS acceptor. When the client authentication config is invalid, the port has
    /// no acceptor and closes every connection.
    pub async fn setup(&mut self, certs: &CertList) -> TlsState {
        self.acceptor = None;
        let verifier = match self.client_verifier(certs) {
            Ok(verifier) => verifier,
            Err(err) => {
                error!(%err, "failed to set up TLS client authentication");
                return TlsState::Error;
            }
        };

        let mut server_config = ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_cert_resolver(server_cert_resolver(certs, self.server_names.clone()));
        server_config
            .alpn_protocols
            .clone_from(&self.alpn_protocols);

        self.acceptor = Some(TlsAcceptor::from(Arc::new(server_config)));
        TlsState::Active
    }

    /// Returns the verifier of the client certificates. `Off` accepts every client.
    pub fn client_verifier(&self, certs: &CertList) -> Result<Arc<dyn ClientCertVerifier>, Error> {
        if self.client_auth.is_off() {
            return Ok(WebPkiClientVerifier::no_client_auth());
        }
        let roots = client_roots(&self.client_ca_certs, certs)?;
        let builder = WebPkiClientVerifier::builder(Arc::new(roots));
        let builder = if self.client_auth == ClientAuthMode::Optional {
            builder.allow_unauthenticated()
        } else {
            builder
        };
        builder.build().map_err(|err| {
            error!(%err, "failed to build the client certificate verifier");
            Error::ClientCaCertsMissing
        })
    }
}

/// Returns the resolver that selects a valid server certificate for the server names.
pub fn server_cert_resolver(
    certs: &CertList,
    default_names: Vec<SubjectName>,
) -> Arc<dyn ResolvesServerCert> {
    Arc::new(CertResolver::new(
        certs
            .iter()
            .filter(|cert| cert.kind == CertKind::Server)
            .cloned()
            .collect(),
        default_names,
        true,
    ))
}

/// Checks the client authentication config of a port.
pub fn validate_client_auth(
    config: &r3v3rs3_api::tls::TlsTermination,
    certs: &CertList,
) -> Result<(), Error> {
    if config.client_auth.is_off() {
        return Ok(());
    }
    client_roots(&config.client_ca_certs, certs).map(|_| ())
}

/// Builds the TLS client config for the upstream servers of a proxy. The config sends the client
/// certificate when the proxy has one. An invalid client certificate is an error, so a proxy never
/// connects without the certificate that it must send.
pub fn upstream_client_config(
    certs: &CertList,
    client_cert: Option<ShortId>,
) -> Result<ClientConfig, Error> {
    let builder = ClientConfig::builder().with_root_certificates(certs.root_certs().clone());
    let Some(id) = client_cert else {
        return Ok(builder.with_no_client_auth());
    };
    let invalid = || Error::InvalidClientCert { id };
    let cert = certs
        .get(id)
        .filter(|cert| cert.kind == CertKind::Client)
        .ok_or_else(invalid)?;
    let chain = cert.certificates().map_err(|_| invalid())?;
    let key = cert.private_key_der().map_err(|_| invalid())?;
    builder.with_client_auth_cert(chain, key).map_err(|err| {
        error!(%id, %err, "failed to load the upstream client certificate");
        invalid()
    })
}

/// Builds the trust store of the client certificates from the selected root certificates.
fn client_roots(ids: &[ShortId], certs: &CertList) -> Result<RootCertStore, Error> {
    if ids.is_empty() {
        return Err(Error::ClientCaCertsMissing);
    }
    let mut roots = RootCertStore::empty();
    for id in ids {
        let cert = certs
            .get(*id)
            .filter(|cert| cert.kind == CertKind::Root)
            .ok_or(Error::InvalidClientCaCert { id: *id })?;
        for der in cert.certificates()? {
            roots.add(der).map_err(|err| {
                error!(%id, %err, "failed to add the client CA certificate");
                Error::InvalidClientCaCert { id: *id }
            })?;
        }
    }
    Ok(roots)
}

/// Returns the TLS acceptor of a port. `None` means that the port needs TLS but has no valid TLS
/// config, so the connection must be closed.
pub(crate) fn port_acceptor(tls: Option<&TlsTermination>) -> Option<Option<TlsAcceptor>> {
    match tls {
        Some(tls) => tls.acceptor.clone().map(Some),
        None => Some(None),
    }
}

/// Combines the TLS acceptor of a port with the TLS config of the active TLS-ALPN-01 challenges.
/// `None` when the TLS config of the port is invalid, so the connection must be closed.
pub(crate) fn port_tls(
    acceptor: Option<Option<TlsAcceptor>>,
    challenge: Option<Arc<ServerConfig>>,
) -> Option<Option<PortTls>> {
    let Some(acceptor) = acceptor else {
        debug!("closing the connection: the TLS config is invalid");
        return None;
    };
    Some(acceptor.map(|acceptor| PortTls::new(acceptor, challenge)))
}

/// The result of the TLS handshake of a port.
pub enum Handshake<IO> {
    Established(Box<TlsStream<IO>>),
    /// The client validated a TLS-ALPN-01 challenge, so the connection carries no data.
    ChallengeServed,
}

impl<IO> Handshake<IO> {
    fn established(stream: TlsStream<IO>) -> Self {
        Self::Established(Box::new(stream))
    }
}

/// The TLS acceptor of a port with the TLS config of the active TLS-ALPN-01 challenges.
#[derive(Clone)]
pub struct PortTls {
    acceptor: TlsAcceptor,
    challenge: Option<Arc<ServerConfig>>,
}

impl PortTls {
    pub fn new(acceptor: TlsAcceptor, challenge: Option<Arc<ServerConfig>>) -> Self {
        Self {
            acceptor,
            challenge,
        }
    }

    /// Runs the handshake. While a challenge is active, a client that offers only `acme-tls/1`
    /// receives the challenge certificate instead of the certificate of the port.
    pub async fn accept<IO>(&self, io: IO) -> io::Result<Handshake<IO>>
    where
        IO: AsyncRead + AsyncWrite + Unpin,
    {
        let Some(challenge) = &self.challenge else {
            return self.acceptor.accept(io).await.map(Handshake::established);
        };
        let start = LazyConfigAcceptor::new(Acceptor::default(), io).await?;
        if !offers_only_acme_tls(&start.client_hello()) {
            let config = self.acceptor.config().clone();
            return start.into_stream(config).await.map(Handshake::established);
        }
        let mut stream = start.into_stream(challenge.clone()).await?;
        if let Err(err) = stream.shutdown().await {
            debug!(%err, "failed to close the TLS-ALPN-01 challenge connection");
        }
        Ok(Handshake::ChallengeServed)
    }
}

/// The verified client certificate of a connection.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClientCertInfo {
    /// The distinguished name of the subject, e.g. `CN=client.example.com`.
    pub subject: String,
    /// The SHA-256 digest of the certificate in hex, in the same format as the certificate list.
    pub fingerprint: String,
}

impl ClientCertInfo {
    /// Reads the leaf certificate of the chain that the client sent.
    pub fn from_chain(chain: Option<&[CertificateDer<'_>]>) -> Option<Self> {
        let der = chain?.first()?.as_ref();
        let (_, x509) = parse_x509_certificate(der).ok()?;
        Some(Self {
            subject: x509.subject().to_string(),
            fingerprint: hex::encode(Sha256::digest(der)),
        })
    }
}

#[derive(Debug, Default)]
pub struct CertResolver {
    certs: Vec<Arc<Cert>>,
    default_names: Vec<SubjectName>,
    sni: bool,
    cache: DashMap<ShortId, Arc<CertifiedKey>>,
}

impl CertResolver {
    pub fn new(certs: Vec<Arc<Cert>>, default_names: Vec<SubjectName>, sni: bool) -> Self {
        Self {
            certs,
            default_names,
            sni,
            cache: DashMap::new(),
        }
    }
}

impl ResolvesServerCert for CertResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        let sni = client_hello
            .server_name()
            .filter(|_| self.sni)
            .map(|sni| SubjectName::DnsName(sni.into()))
            .into_iter()
            .collect::<Vec<_>>();

        let names = if sni.is_empty() {
            &self.default_names
        } else {
            &sni
        };

        let cert = self
            .certs
            .iter()
            .find(|cert| cert.is_valid() && names.iter().all(|name| cert.has_subject_name(name)))?;

        if let Some(cert) = self.cache.get(&cert.id()) {
            Some(cert.clone())
        } else {
            let certified = match cert.certified_key() {
                Ok(certified) => Arc::new(certified),
                Err(err) => {
                    error!("failed to load certified key: {}", err);
                    return None;
                }
            };
            self.cache.insert(cert.id(), certified.clone());
            Some(certified)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certs::alpn::{challenge_config, ChallengeCerts, TlsAlpnChallenge, ACME_TLS_ALPN};
    use r3v3rs3_api::tls::TlsTermination as TlsConfig;
    use tokio::io::DuplexStream;
    use tokio_rustls::rustls::pki_types::ServerName;

    /// Returns the TLS of a port with a certificate for `localhost`, and that certificate.
    fn port_tls(challenges: &[TlsAlpnChallenge]) -> (PortTls, CertificateDer<'static>) {
        let root = Cert::new_ca().unwrap();
        let server =
            Arc::new(Cert::new_self_signed(&["localhost".parse().unwrap()], &root).unwrap());
        let resolver = CertResolver::new(vec![server.clone()], vec![], true);
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));
        let challenge = (!challenges.is_empty())
            .then(|| challenge_config(Arc::new(ChallengeCerts::new(challenges).unwrap())));
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let cert = server.certificates().unwrap().remove(0);
        (PortTls::new(acceptor, challenge), cert)
    }

    /// Runs the handshake of a client that offers the ALPN protocols. Returns the result of the
    /// port, the certificate that the client received and the negotiated ALPN protocol.
    async fn handshake(
        tls: &PortTls,
        alpn: &[&[u8]],
    ) -> (
        Handshake<DuplexStream>,
        CertificateDer<'static>,
        Option<Vec<u8>>,
    ) {
        let connector = super::testing::connector(alpn);
        let (client_io, server_io) = tokio::io::duplex(16384);
        let name = ServerName::try_from("localhost").unwrap();
        let (server, client) =
            tokio::join!(tls.accept(server_io), connector.connect(name, client_io));
        let client = client.unwrap();
        let conn = &client.get_ref().1;
        let cert = conn.peer_certificates().unwrap()[0].clone();
        (
            server.unwrap(),
            cert,
            conn.alpn_protocol().map(<[u8]>::to_vec),
        )
    }

    fn config(client_auth: ClientAuthMode, client_ca_certs: Vec<ShortId>) -> TlsConfig {
        TlsConfig {
            client_auth,
            client_ca_certs,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn client_auth_needs_root_certificates() {
        let root = Arc::new(Cert::new_ca().unwrap());
        let server =
            Arc::new(Cert::new_self_signed(&["localhost".parse().unwrap()], &root).unwrap());
        let certs = CertList::new([root.clone(), server.clone()]).await;

        let off = config(ClientAuthMode::Off, vec![server.id]);
        assert!(validate_client_auth(&off, &certs).is_ok());

        let missing = config(ClientAuthMode::Required, vec![]);
        assert!(matches!(
            validate_client_auth(&missing, &certs),
            Err(Error::ClientCaCertsMissing)
        ));

        let not_root = config(ClientAuthMode::Optional, vec![server.id]);
        assert!(matches!(
            validate_client_auth(&not_root, &certs),
            Err(Error::InvalidClientCaCert { id }) if id == server.id
        ));

        let valid = config(ClientAuthMode::Required, vec![root.id]);
        assert!(validate_client_auth(&valid, &certs).is_ok());
        let mut tls = TlsTermination::new(&valid, vec![]).unwrap();
        assert_eq!(tls.setup(&certs).await, TlsState::Active);
        assert!(tls.acceptor.is_some());

        let mut tls = TlsTermination::new(&missing, vec![]).unwrap();
        assert_eq!(tls.setup(&certs).await, TlsState::Error);
        assert!(port_acceptor(Some(&tls)).is_none());
        assert!(matches!(port_acceptor(None), Some(None)));
    }

    #[tokio::test]
    async fn upstream_client_config_needs_a_client_certificate() {
        let root = Arc::new(Cert::new_ca().unwrap());
        let server =
            Arc::new(Cert::new_self_signed(&["localhost".parse().unwrap()], &root).unwrap());
        let client =
            Arc::new(Cert::new_client(&["client.example.com".parse().unwrap()], &root).unwrap());
        let certs = CertList::new([root.clone(), server.clone(), client.clone()]).await;

        let config = upstream_client_config(&certs, None).unwrap();
        assert!(!config.client_auth_cert_resolver.has_certs());
        let config = upstream_client_config(&certs, Some(client.id)).unwrap();
        assert!(config.client_auth_cert_resolver.has_certs());

        for id in [server.id, root.id, "a1b2c3d".parse().unwrap()] {
            assert!(matches!(
                upstream_client_config(&certs, Some(id)),
                Err(Error::InvalidClientCert { id: err_id }) if err_id == id
            ));
        }
    }

    #[test]
    fn client_cert_info_reads_the_leaf_certificate() {
        let root = Cert::new_ca().unwrap();
        let client = Cert::new_client(&["client.example.com".parse().unwrap()], &root).unwrap();
        let chain = client.certificates().unwrap();

        let info = ClientCertInfo::from_chain(Some(&chain)).unwrap();
        assert_eq!(info.subject, "CN=client.example.com");
        assert_eq!(info.fingerprint, client.fingerprint);
        assert_eq!(ClientCertInfo::from_chain(None), None);
        assert_eq!(ClientCertInfo::from_chain(Some(&[])), None);
    }

    #[tokio::test]
    async fn only_an_acme_tls_client_receives_the_challenge_certificate() {
        let challenge = TlsAlpnChallenge {
            domain: "localhost".to_string(),
            digest: [9; 32],
        };
        let (tls, port_cert) = port_tls(&[challenge]);

        let (result, cert, alpn) = handshake(&tls, &[ACME_TLS_ALPN]).await;
        assert!(matches!(result, Handshake::ChallengeServed));
        assert_ne!(cert, port_cert);
        assert_eq!(alpn.as_deref(), Some(ACME_TLS_ALPN));
        let (_, x509) = parse_x509_certificate(cert.as_ref()).unwrap();
        assert!(x509
            .extensions()
            .iter()
            .any(|ext| ext.critical && ext.oid.to_id_string() == "1.3.6.1.5.5.7.1.31"));

        let (result, cert, alpn) = handshake(&tls, &[b"h2", ACME_TLS_ALPN]).await;
        assert!(matches!(result, Handshake::Established(_)));
        assert_eq!(cert, port_cert);
        assert_eq!(alpn, None);

        let (tls, port_cert) = port_tls(&[]);
        let (result, cert, _) = handshake(&tls, &[ACME_TLS_ALPN]).await;
        assert!(matches!(result, Handshake::Established(_)));
        assert_eq!(cert, port_cert);
    }
}

/// TLS clients for the tests.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Arc;
    use tokio_rustls::rustls::client::danger::{
        HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
    };
    use tokio_rustls::rustls::crypto::ring;
    use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, Error, SignatureScheme};
    use tokio_rustls::TlsConnector;

    /// Accepts every server certificate, so a test can read a challenge certificate.
    #[derive(Debug)]
    struct AcceptAnyCert;

    impl ServerCertVerifier for AcceptAnyCert {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    /// A client that offers the ALPN protocols and accepts every server certificate.
    pub(crate) fn connector(alpn: &[&[u8]]) -> TlsConnector {
        let mut config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        TlsConnector::from(Arc::new(config))
    }
}
