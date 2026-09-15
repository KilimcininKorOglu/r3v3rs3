use pkcs8::SecretDocument;
use r3v3rs3_api::cert::{CertInfo, CertKind, CertMetadata};
use r3v3rs3_api::discovery::DiscoverySource;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::subject_name::SubjectName;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, Ia5String, IsCa, KeyPair, SanType,
};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{BufRead, BufReader};
use std::net::IpAddr;
use std::str::FromStr;
use tokio_rustls::rustls::crypto::ring::sign;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::sign::CertifiedKey;
use tracing::error;
use x509_parser::{extensions::GeneralName, time::ASN1Time};
use x509_parser::{parse_x509_certificate, prelude::X509Certificate};

pub mod acme;
pub mod alpn;
pub mod challenges;
pub mod dns;

#[derive(Clone)]
pub struct Cert {
    pub id: ShortId,
    pub kind: CertKind,
    pub key: Option<SecretDocument>,
    pub pem_chain: Vec<u8>,
    pub pem_key: Option<Vec<u8>>,
    pub fingerprint: String,
    pub issuer: String,
    pub root_cert: Option<String>,
    pub san: Vec<SubjectName>,
    pub not_after: ASN1Time,
    pub not_before: ASN1Time,
    pub is_ca: bool,
    pub metadata: Option<CertMetadata>,
    /// The discovery provider that read the certificate. A discovered certificate is not saved.
    pub source: Option<DiscoverySource>,
}

impl PartialEq for Cert {
    fn eq(&self, other: &Self) -> bool {
        self.fingerprint == other.fingerprint
    }
}

impl Eq for Cert {}

impl fmt::Debug for Cert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cert")
            .field("id", &self.id)
            .field("fingerprint", &self.fingerprint)
            .field("issuer", &self.issuer)
            .field("root_cert", &self.root_cert)
            .field("san", &self.san)
            .field("not_after", &self.not_after)
            .field("not_before", &self.not_before)
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl PartialOrd for Cert {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(
            other
                .not_before
                .cmp(&self.not_before)
                .then_with(|| self.not_after.cmp(&other.not_after))
                .then_with(|| self.fingerprint.cmp(&other.fingerprint)),
        )
    }
}

impl Cert {
    pub fn id(&self) -> ShortId {
        self.id
    }

    pub fn info(&self) -> CertInfo {
        CertInfo {
            id: self.id,
            kind: self.kind,
            fingerprint: self.fingerprint.clone(),
            issuer: self.issuer.clone(),
            root_cert: self.root_cert.clone(),
            san: self.san.clone(),
            not_after: self.not_after.timestamp(),
            not_before: self.not_before.timestamp(),
            is_ca: self.is_ca,
            has_private_key: self.key.is_some(),
            metadata: self.metadata.clone(),
            source: self.source.clone(),
        }
    }

    pub fn is_valid(&self) -> bool {
        let now = ASN1Time::now();
        self.not_before <= now && now <= self.not_after
    }

    pub fn has_subject_name(&self, name: &SubjectName) -> bool {
        for san in &self.san {
            if match (san, name) {
                (SubjectName::DnsName(c), SubjectName::DnsName(n)) => c == n,
                (SubjectName::WildcardDnsName(c), SubjectName::DnsName(n)) => {
                    c == n.trim_start_matches(|c| c != '.').trim_start_matches('.')
                }
                (SubjectName::WildcardDnsName(c), SubjectName::WildcardDnsName(n)) => c == n,
                (SubjectName::IPAddress(c), SubjectName::IPAddress(n)) => c == n,
                _ => false,
            } {
                return true;
            }
        }
        false
    }

    pub fn new(
        kind: CertKind,
        pem_chain: Vec<u8>,
        pem_key: Option<Vec<u8>>,
    ) -> Result<Self, Error> {
        let key = pem_key
            .as_deref()
            .map(|pem_key| {
                let key = parse_private_key(pem_key)?;
                SecretDocument::try_from(key.secret_der())
                    .map_err(|_| Error::FailedToReadPrivateKey)
            })
            .transpose()?;
        let chain_meta = pem_chain.as_slice();
        let mut meta_read = BufReader::new(chain_meta);
        let mut comment = String::new();
        meta_read
            .read_line(&mut comment)
            .map_err(|_| Error::FailedToReadCertificate)?;

        let metadata: Option<CertMetadata> = serde_qs::from_str(
            comment
                .trim_start_matches(|c: char| c == '#' || c.is_whitespace())
                .trim_end(),
        )
        .ok();

        let mut chain = pem_chain.as_slice();
        let certs = rustls_pemfile::certs(&mut chain)
            .map(|cert| cert.map_err(|_| Error::FailedToReadCertificate))
            .collect::<Result<Vec<_>, _>>()?;

        let der = certs
            .first()
            .ok_or(Error::FailedToReadCertificate)?
            .as_ref();
        let mut hasher = Sha256::new();
        hasher.update(der);
        let id = hasher.finalize();
        let mut short_id = [0; 7];
        short_id.copy_from_slice(&id[..7]);
        let fingerprint = hex::encode(id);

        let parsed_chain = parse_chain(&certs)?;
        let x509 = parsed_chain.first().ok_or(Error::FailedToReadCertificate)?;

        let common_name = x509.subject().iter_common_name().find_map(|name| {
            name.as_str()
                .ok()
                .and_then(|name| SubjectName::from_str(name).ok())
        });

        let san = common_name
            .clone()
            .into_iter()
            .chain(
                x509.subject_alternative_name()
                    .into_iter()
                    .flatten()
                    .flat_map(|name| &name.value.general_names)
                    .filter_map(|name| match name {
                        GeneralName::DNSName(name) => SubjectName::from_str(name).ok(),
                        GeneralName::IPAddress(ip) => match ip.len() {
                            4 => {
                                let addr = [ip[0], ip[1], ip[2], ip[3]];
                                Some(SubjectName::IPAddress(IpAddr::V4(addr.into())))
                            }
                            16 => {
                                let mut addr = [0; 16];
                                addr.copy_from_slice(ip);
                                Some(SubjectName::IPAddress(IpAddr::V6(addr.into())))
                            }
                            _ => None,
                        },
                        _ => None,
                    })
                    .filter(|name| Some(name) != common_name.as_ref()),
            )
            .collect();

        let not_after = x509.validity().not_after;
        let not_before = x509.validity().not_before;
        let is_ca = x509.is_ca();

        let issuer = x509.issuer().to_string();
        let root_cert = parsed_chain
            .last()
            .filter(|_| chain.len() > 1)
            .map(|cert| cert.subject().to_string());

        Ok(Self {
            id: short_id.into(),
            kind,
            fingerprint,
            key,
            pem_chain,
            pem_key,
            issuer,
            root_cert,
            san,
            not_after,
            not_before,
            is_ca,
            metadata,
            source: None,
        })
    }

    pub fn new_ca() -> Result<Self, Error> {
        let mut distinguished_name = DistinguishedName::new();
        distinguished_name.push(DnType::CommonName, "r3v3rs3 CA");
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.distinguished_name = distinguished_name;

        let keypair =
            KeyPair::generate().map_err(|_| Error::FailedToGenerateSelfSignedCertificate)?;
        let cert = params.self_signed(&keypair).map_err(generation_error)?;

        let pem_chain = cert.pem().into_bytes();
        let pem_key = keypair.serialize_pem().into_bytes();
        Self::new(CertKind::Root, pem_chain, Some(pem_key))
    }

    /// Creates a server certificate signed by `ca`.
    pub fn new_self_signed(san: &[SubjectName], ca: &Cert) -> Result<Self, Error> {
        Self::new_signed(CertKind::Server, san, ca)
    }

    /// Creates a client certificate with the `clientAuth` extended key usage, signed by `ca`.
    pub fn new_client(san: &[SubjectName], ca: &Cert) -> Result<Self, Error> {
        Self::new_signed(CertKind::Client, san, ca)
    }

    fn new_signed(kind: CertKind, san: &[SubjectName], ca: &Cert) -> Result<Self, Error> {
        let (ca_cert, ca_keypair) = ca.signing_cert()?;

        let mut params = leaf_params(san)?;
        if kind == CertKind::Client {
            params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        }

        let keypair =
            KeyPair::generate().map_err(|_| Error::FailedToGenerateSelfSignedCertificate)?;
        let cert = params
            .signed_by(&keypair, &ca_cert, &ca_keypair)
            .map_err(generation_error)?;

        let pem_chain = format!("{}\r\n{}", cert.pem(), ca_cert.pem()).into_bytes();
        let pem_key = keypair.serialize_pem().into_bytes();
        Self::new(kind, pem_chain, Some(pem_key))
    }

    /// Rebuilds this CA certificate and its key pair, so that they can sign a new certificate.
    fn signing_cert(&self) -> Result<(Certificate, KeyPair), Error> {
        let ca_pem =
            std::str::from_utf8(&self.pem_chain).map_err(|_| Error::FailedToReadPrivateKey)?;
        let pem_key = self.pem_key.as_ref().ok_or(Error::FailedToReadPrivateKey)?;
        let key_pem = std::str::from_utf8(pem_key).map_err(|_| Error::FailedToReadPrivateKey)?;
        let ca_keypair = KeyPair::from_pem(key_pem).map_err(|_| Error::FailedToReadPrivateKey)?;
        let ca_params = CertificateParams::from_ca_cert_pem(ca_pem)
            .map_err(|_| Error::FailedToGenerateSelfSignedCertificate)?;
        let ca_cert = ca_params
            .self_signed(&ca_keypair)
            .map_err(generation_error)?;
        Ok((ca_cert, ca_keypair))
    }

    pub fn certified_key(&self) -> Result<CertifiedKey, Error> {
        match self.certified_impl() {
            Ok(certified) => Ok(certified),
            Err(err) => {
                error!(%err);
                Err(Error::FailedToReadPrivateKey)
            }
        }
    }

    pub fn certificates(&self) -> Result<Vec<CertificateDer<'static>>, Error> {
        let mut chain = self.pem_chain.as_slice();
        rustls_pemfile::certs(&mut chain)
            .map(|cert| {
                cert.map(|cert| cert.to_owned())
                    .map_err(|_| Error::FailedToReadCertificate)
            })
            .collect()
    }

    /// Returns the private key in DER with its format: PKCS#8, PKCS#1 or SEC1.
    pub fn private_key_der(&self) -> Result<PrivateKeyDer<'static>, Error> {
        let pem_key = self
            .pem_key
            .as_deref()
            .ok_or(Error::FailedToReadPrivateKey)?;
        parse_private_key(pem_key)
    }

    fn certified_impl(&self) -> anyhow::Result<CertifiedKey> {
        let key = self.private_key_der()?;
        let signing_key = sign::any_supported_type(&key).map_err(|err| anyhow::anyhow!("{err}"))?;
        let chain = self.certificates()?;
        Ok(CertifiedKey::new(chain, signing_key))
    }
}

/// Returns the parameters of a leaf certificate with the subject alternative names. The first name is the common name.
fn leaf_params(san: &[SubjectName]) -> Result<CertificateParams, Error> {
    let mut params = CertificateParams::default();
    for name in san {
        let name = if let SubjectName::IPAddress(ip) = name {
            SanType::IpAddress(*ip)
        } else {
            let name = Ia5String::from_str(&name.to_string())
                .map_err(|_| Error::FailedToGenerateSelfSignedCertificate)?;
            SanType::DnsName(name)
        };
        params.subject_alt_names.push(name);
    }

    let common_name = san
        .iter()
        .map(|name| name.to_string())
        .next()
        .unwrap_or_else(|| "r3v3rs3 Cert".into());
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, common_name);
    params.distinguished_name = distinguished_name;
    Ok(params)
}

fn generation_error(err: rcgen::Error) -> Error {
    error!(%err);
    Error::FailedToGenerateSelfSignedCertificate
}

/// Reads the first private key of a PEM text. rustls signs with PKCS#8, PKCS#1 (RSA) and SEC1 (EC)
/// keys.
fn parse_private_key(mut pem_key: &[u8]) -> Result<PrivateKeyDer<'static>, Error> {
    match rustls_pemfile::private_key(&mut pem_key) {
        Ok(Some(key)) => Ok(key),
        Ok(None) | Err(_) => Err(Error::FailedToReadPrivateKey),
    }
}

fn parse_chain<'a>(chain: &'a [CertificateDer]) -> Result<Vec<X509Certificate<'a>>, Error> {
    chain
        .iter()
        .map(|data| {
            parse_x509_certificate(data.as_ref())
                .map(|(_, cert)| cert)
                .map_err(|_| Error::FailedToReadCertificate)
        })
        .collect()
}

#[cfg(test)]
mod test {
    use super::*;
    use pkcs8::PrivateKeyInfo;

    type Sign = fn(&[SubjectName], &Cert) -> Result<Cert, Error>;

    /// Signs a certificate for `name` and returns it with its `clientAuth` extended key usage flag.
    fn signed(sign: Sign, name: &str) -> (Cert, Option<bool>) {
        let san = [SubjectName::from_str(name).unwrap()];
        let ca = Cert::new_ca().unwrap();
        let cert = sign(&san, &ca).unwrap();
        assert_eq!(cert.san, san);
        assert!(cert.key.is_some());

        let chain = cert.certificates().unwrap();
        let (_, x509) = parse_x509_certificate(chain[0].as_ref()).unwrap();
        let client_auth = x509
            .extended_key_usage()
            .unwrap()
            .map(|eku| eku.value.client_auth);
        (cert, client_auth)
    }

    #[test]
    fn test_self_signed() {
        let (cert, client_auth) = signed(Cert::new_self_signed, "localhost");
        assert_eq!(cert.kind, CertKind::Server);
        assert_eq!(client_auth, None);
    }

    #[test]
    fn client_certificate_has_the_client_auth_usage() {
        let (cert, client_auth) = signed(Cert::new_client, "client.example.com");
        assert_eq!(cert.kind, CertKind::Client);
        assert_eq!(client_auth, Some(true));
    }

    /// OpenSSL and cert-manager write EC keys as SEC1 `EC PRIVATE KEY` blocks. The private key
    /// bytes of a PKCS#8 EC key are its SEC1 key.
    #[test]
    fn a_sec1_private_key_signs_like_its_pkcs8_key() {
        let (cert, _) = signed(Cert::new_self_signed, "sec1.example.com");
        let pkcs8 = cert.key.as_ref().unwrap();
        let info = pkcs8.decode_msg::<PrivateKeyInfo>().unwrap();
        let line_ending = pkcs8::der::pem::LineEnding::LF;
        let sec1 = pkcs8::der::pem::encode_string("EC PRIVATE KEY", line_ending, info.private_key)
            .unwrap();

        let reloaded = Cert::new(
            CertKind::Server,
            cert.pem_chain.clone(),
            Some(sec1.into_bytes()),
        )
        .unwrap();
        assert!(matches!(
            reloaded.private_key_der().unwrap(),
            PrivateKeyDer::Sec1(_)
        ));
        assert!(reloaded.certified_key().is_ok());
    }

    #[test]
    fn a_text_without_a_private_key_is_rejected() {
        let (cert, _) = signed(Cert::new_self_signed, "nokey.example.com");
        let result = Cert::new(
            CertKind::Server,
            cert.pem_chain.clone(),
            Some(cert.pem_chain.clone()),
        );
        assert!(matches!(result, Err(Error::FailedToReadPrivateKey)));
    }
}
