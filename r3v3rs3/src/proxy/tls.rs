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
use std::str::FromStr;
use std::sync::Arc;
use tokio_rustls::rustls::pki_types::CertificateDer;
use tokio_rustls::rustls::server::danger::ClientCertVerifier;
use tokio_rustls::rustls::server::{ClientHello, ResolvesServerCert, WebPkiClientVerifier};
use tokio_rustls::rustls::sign::CertifiedKey;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::TlsAcceptor;
use tracing::error;
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
    use r3v3rs3_api::tls::TlsTermination as TlsConfig;

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
}
