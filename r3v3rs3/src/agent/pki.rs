//! The certificate authority of the agent link. The master keeps its own CA under
//! `certs/agent/` of the config directory, presents a server certificate that this CA signed, and
//! signs the client certificate of every enrolled agent.

use crate::certs::Cert;
use crate::config::file::write_private;
use anyhow::{Context, anyhow};
use r3v3rs3_api::cert::CertKind;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::subject_name::SubjectName;
use rcgen::{
    CertificateParams, CertificateSigningRequestParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, KeyPair, SanType, string::Ia5String,
};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::fs;
use tokio_rustls::rustls::client::WebPkiServerVerifier;
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::{
    CryptoProvider, ring, verify_tls12_signature, verify_tls13_signature,
};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::server::WebPkiClientVerifier;
use tokio_rustls::rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, Error, RootCertStore, ServerConfig,
    SignatureScheme,
};

/// The name in the server certificate of the master. The agent checks this name instead of the
/// host name that it dials, because the certificate comes from the private CA of the link.
pub const MASTER_NAME: &str = "r3v3rs3-master";

/// The ALPN protocol of the agent link.
pub const ALPN: &[u8] = b"r3v3rs3-agent/1";

/// The CA and the server certificate of the master.
pub struct AgentPki {
    ca: Cert,
    server: Cert,
}

impl AgentPki {
    /// Loads the CA and the server certificate from `certs/agent/` of `config_dir`, and creates
    /// the missing ones.
    pub async fn load_or_create(config_dir: &Path) -> anyhow::Result<Self> {
        let dir = config_dir.join("certs").join("agent");
        fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("cannot create {}", dir.display()))?;
        let ca = load_or_create(&dir, "ca", CertKind::Root, Cert::new_ca).await?;
        let master = SubjectName::from_str(MASTER_NAME)?;
        let server = load_or_create(&dir, "server", CertKind::Server, || {
            Cert::new_self_signed(&[master], &ca)
        })
        .await?;
        Ok(Self { ca, server })
    }

    /// The SHA-256 of the CA certificate in lowercase hex. An enrollment token carries it, so the
    /// agent recognizes the master at its first connection.
    pub fn ca_hash(&self) -> &str {
        &self.ca.fingerprint
    }

    /// The CA certificate in PEM.
    pub fn ca_pem(&self) -> anyhow::Result<String> {
        Ok(String::from_utf8(self.ca.pem_chain.clone())?)
    }

    /// Signs the certificate signing request of an agent. The master sets the subject and the
    /// usage of the certificate, so only the public key comes from the request.
    pub fn sign(&self, csr_pem: &str, target: ShortId) -> anyhow::Result<String> {
        let request = CertificateSigningRequestParams::from_pem(csr_pem)
            .map_err(|err| anyhow!("invalid certificate signing request: {err}"))?;
        let request = CertificateSigningRequestParams {
            params: agent_params(target)?,
            public_key: request.public_key,
        };
        let certificate = request.signed_by(&self.ca.issuer()?)?;
        Ok(certificate.pem())
    }

    /// The TLS configuration of the agent port. A client certificate is optional, because the
    /// enrollment connection has none. The listener allows such a connection only the enrollment.
    pub fn server_config(&self) -> anyhow::Result<ServerConfig> {
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots(&self.ca.certificates()?)?))
            .allow_unauthenticated()
            .build()?;
        let mut config = ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(self.server.certificates()?, self.server.private_key_der()?)?;
        config.alpn_protocols = vec![ALPN.to_vec()];
        Ok(config)
    }
}

/// The certificate parameters of an agent: its name and the client authentication usage only, so
/// the certificate cannot serve as the server certificate of a master.
fn agent_params(target: ShortId) -> anyhow::Result<CertificateParams> {
    let name = format!("agent-{target}");
    let mut params = CertificateParams::default();
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, name.clone());
    params.distinguished_name = distinguished_name;
    params.subject_alt_names = vec![SanType::DnsName(Ia5String::from_str(&name)?)];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    Ok(params)
}

async fn load_or_create(
    dir: &Path,
    name: &str,
    kind: CertKind,
    create: impl FnOnce() -> Result<Cert, r3v3rs3_api::error::Error>,
) -> anyhow::Result<Cert> {
    let chain_path = dir.join(format!("{name}.pem"));
    let key_path = dir.join(format!("{name}.key"));
    if fs::try_exists(&chain_path).await? {
        return load(&chain_path, &key_path, kind)
            .await
            .with_context(|| format!("cannot load {}", chain_path.display()));
    }
    let cert = create()?;
    save(&cert, &chain_path, &key_path)
        .await
        .with_context(|| format!("cannot write {}", chain_path.display()))?;
    Ok(cert)
}

async fn load(chain_path: &Path, key_path: &Path, kind: CertKind) -> anyhow::Result<Cert> {
    let chain = fs::read(chain_path).await?;
    let key = fs::read(key_path).await?;
    Ok(Cert::new(kind, chain, Some(key))?)
}

async fn save(cert: &Cert, chain_path: &Path, key_path: &Path) -> anyhow::Result<()> {
    let key = cert.pem_key.clone().context("the certificate has no key")?;
    write_private(key_path, String::from_utf8(key)?).await?;
    fs::write(chain_path, &cert.pem_chain).await?;
    Ok(())
}

/// The SHA-256 of a DER certificate in lowercase hex.
pub fn fingerprint(der: &[u8]) -> String {
    hex::encode(Sha256::digest(der))
}

fn roots(certificates: &[CertificateDer<'static>]) -> anyhow::Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for certificate in certificates {
        roots.add(certificate.clone())?;
    }
    Ok(roots)
}

fn pem_certificates(pem: &str) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let certificates = rustls_pemfile::certs(&mut pem.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    if certificates.is_empty() {
        anyhow::bail!("the PEM text holds no certificate");
    }
    Ok(certificates)
}

/// A new key of an agent in PEM, with a certificate signing request for it.
pub fn new_agent_key() -> anyhow::Result<(String, String)> {
    let key = KeyPair::generate()?;
    let request = CertificateParams::default().serialize_request(&key)?;
    Ok((key.serialize_pem(), request.pem()?))
}

/// The TLS configuration of the first connection of an agent. It has no client certificate, and
/// it trusts the master only when the CA at the end of its chain has the SHA-256 of the token.
pub fn enrollment_client_config(ca_hash: &str) -> ClientConfig {
    let verifier = PinnedCa {
        ca_hash: ca_hash.to_string(),
        provider: Arc::new(ring::default_provider()),
    };
    let mut config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    config.alpn_protocols = vec![ALPN.to_vec()];
    config
}

/// The TLS configuration of an enrolled agent: it trusts the stored CA and presents its own
/// certificate.
pub fn agent_client_config(
    ca_pem: &str,
    certificate_pem: &str,
    key_pem: &str,
) -> anyhow::Result<ClientConfig> {
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())?
        .context("the agent key file holds no key")?;
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots(&pem_certificates(ca_pem)?)?)
        .with_client_auth_cert(pem_certificates(certificate_pem)?, key)?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(config)
}

/// The server name that an agent checks in the certificate of the master.
pub fn master_server_name() -> anyhow::Result<ServerName<'static>> {
    Ok(ServerName::try_from(MASTER_NAME)?)
}

/// Trusts a server whose chain ends in the CA with a known SHA-256, then verifies the chain with
/// that CA as the only root.
#[derive(Debug)]
struct PinnedCa {
    ca_hash: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedCa {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let (ca, rest) = intermediates
            .split_last()
            .ok_or(Error::InvalidCertificate(CertificateError::UnknownIssuer))?;
        let matches: bool = fingerprint(ca)
            .as_bytes()
            .ct_eq(self.ca_hash.as_bytes())
            .into();
        if !matches {
            return Err(Error::InvalidCertificate(CertificateError::UnknownIssuer));
        }
        let mut roots = RootCertStore::empty();
        roots.add(ca.clone().into_owned())?;
        let verifier =
            WebPkiServerVerifier::builder_with_provider(Arc::new(roots), self.provider.clone())
                .build()
                .map_err(|err| Error::General(err.to_string()))?;
        verifier.verify_server_cert(end_entity, rest, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The private key of a PEM text, for the tests of the link.
#[cfg(test)]
pub(crate) fn private_key(
    pem: &str,
) -> anyhow::Result<tokio_rustls::rustls::pki_types::PrivateKeyDer<'static>> {
    rustls_pemfile::private_key(&mut pem.as_bytes())?.context("no key")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("r3v3rs3-agent-pki-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Runs one handshake over a pipe and returns the fingerprint of the client certificate that
    /// the server received, if any.
    async fn handshake(
        server: ServerConfig,
        client: ClientConfig,
    ) -> anyhow::Result<Option<String>> {
        let (client_io, server_io) = duplex(16384);
        let acceptor = TlsAcceptor::from(Arc::new(server));
        let connector = TlsConnector::from(Arc::new(client));
        let server = async {
            let mut stream = acceptor.accept(server_io).await?;
            let peer = stream
                .get_ref()
                .1
                .peer_certificates()
                .and_then(|chain| chain.first())
                .map(|der| fingerprint(der));
            stream.write_all(b"ok").await?;
            stream.flush().await?;
            anyhow::Ok(peer)
        };
        let client = async {
            let mut stream = connector.connect(master_server_name()?, client_io).await?;
            let mut buf = [0; 2];
            stream.read_exact(&mut buf).await?;
            anyhow::Ok(())
        };
        let (server, client) = tokio::join!(server, client);
        client?;
        server
    }

    async fn enrolled(pki: &AgentPki, target: ShortId) -> (String, String) {
        let (key, request) = new_agent_key().unwrap();
        (pki.sign(&request, target).unwrap(), key)
    }

    #[tokio::test]
    async fn the_ca_is_created_once_and_its_key_is_private() {
        let dir = temp_dir();
        let first = AgentPki::load_or_create(&dir).await.unwrap();
        let second = AgentPki::load_or_create(&dir).await.unwrap();
        assert_eq!(first.ca_hash(), second.ca_hash());
        assert_eq!(first.ca_hash().len(), 64);
        for key in ["ca.key", "server.key"] {
            let mode = std::fs::metadata(dir.join("certs/agent").join(key))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{key}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_agent_trusts_the_master_only_with_the_hash_of_its_ca() {
        let dir = temp_dir();
        let pki = AgentPki::load_or_create(&dir).await.unwrap();
        let server = pki.server_config().unwrap();
        let peer = handshake(server.clone(), enrollment_client_config(pki.ca_hash()))
            .await
            .unwrap();
        assert_eq!(peer, None);
        let wrong = "0".repeat(64);
        assert!(
            handshake(server, enrollment_client_config(&wrong))
                .await
                .is_err()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_enrolled_agent_presents_the_signed_certificate() {
        let dir = temp_dir();
        let pki = AgentPki::load_or_create(&dir).await.unwrap();
        let target = ShortId::new();
        let (certificate, key) = enrolled(&pki, target).await;
        let client = agent_client_config(&pki.ca_pem().unwrap(), &certificate, &key).unwrap();
        let peer = handshake(pki.server_config().unwrap(), client)
            .await
            .unwrap();
        let der = pem_certificates(&certificate).unwrap();
        assert_eq!(peer, Some(fingerprint(&der[0])));

        let (_, parsed) = x509_parser::parse_x509_certificate(&der[0]).unwrap();
        assert_eq!(
            parsed
                .subject()
                .iter_common_name()
                .next()
                .unwrap()
                .as_str()
                .unwrap(),
            format!("agent-{target}")
        );
        let usage = parsed.extended_key_usage().unwrap().unwrap().value;
        assert!(usage.client_auth && !usage.server_auth);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn the_certificate_of_an_agent_cannot_pose_as_the_master() {
        let dir = temp_dir();
        let pki = AgentPki::load_or_create(&dir).await.unwrap();
        let (certificate, key) = enrolled(&pki, ShortId::new()).await;
        let mut chain = pem_certificates(&certificate).unwrap();
        chain.extend(pem_certificates(&pki.ca_pem().unwrap()).unwrap());
        let fake_master = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(chain, private_key(&key).unwrap())
            .unwrap();
        let (other, other_key) = enrolled(&pki, ShortId::new()).await;
        let victim = agent_client_config(&pki.ca_pem().unwrap(), &other, &other_key).unwrap();
        assert!(handshake(fake_master.clone(), victim).await.is_err());
        let enrolling = enrollment_client_config(pki.ca_hash());
        assert!(handshake(fake_master, enrolling).await.is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn the_master_refuses_a_certificate_of_another_ca() {
        let dir = temp_dir();
        let pki = AgentPki::load_or_create(&dir).await.unwrap();
        let other_dir = temp_dir();
        let other = AgentPki::load_or_create(&other_dir).await.unwrap();
        let (certificate, key) = enrolled(&other, ShortId::new()).await;
        let client = agent_client_config(&pki.ca_pem().unwrap(), &certificate, &key).unwrap();
        assert!(
            handshake(pki.server_config().unwrap(), client)
                .await
                .is_err()
        );
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(other_dir).unwrap();
    }
}
