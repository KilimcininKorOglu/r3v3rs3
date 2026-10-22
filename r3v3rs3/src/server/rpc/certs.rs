use super::RpcMethod;
use crate::{
    accounts::Permission, audit::AuditRecord, certs::Cert, server::cert_list::CertList,
    server::state::ServerState,
};
use flate2::{Compression, write::GzEncoder};
use hyper::body::Bytes;
use r3v3rs3_api::{
    audit::AuditAction,
    cert::{CertInfo, CertKind, DeleteCertResult, DeleteCertStatus, MAX_DELETE_CERTS},
    cert_coverage::{DomainCoverageInfo, DomainSslStatus, extract_root_domain},
    error::Error,
    id::ShortId,
    subject_name::SubjectName,
};
use std::{collections::HashSet, sync::Arc, time::SystemTime};
use tar::Header;
use tracing::error;

pub struct GetCertList;

#[async_trait::async_trait]
impl RpcMethod for GetCertList {
    type Output = Vec<CertInfo>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.certs.iter().map(|item| item.info()).collect())
    }
}

pub struct GetCert {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for GetCert {
    type Output = Arc<Cert>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state.certs.get(self.id).cloned().ok_or(Error::IdNotFound {
            id: self.id.to_string(),
        })
    }
}

pub struct AddCert {
    pub cert: Arc<Cert>,
}

#[async_trait::async_trait]
impl RpcMethod for AddCert {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state.storage.save_cert(&self.cert).await?;
        state.certs.add(self.cert);
        state.update_certs().await;
        state.reload_proxies().await;
        Ok(())
    }

    fn audit(&self) -> Option<AuditRecord> {
        let names = self.cert.san.iter().map(ToString::to_string);
        let record = AuditRecord::new(AuditAction::AddCert).id(self.cert.id);
        Some(record.summary(names.collect::<Vec<_>>().join(", ")))
    }
}

pub struct DeleteCert {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for DeleteCert {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        remove_cert(state, self.id).await?;
        state.update_certs().await;
        state.reload_proxies().await;
        Ok(())
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::DeleteCert).id(self.id))
    }
}

/// Deletes each certificate that nothing uses. The output tells what happened to each id, once
/// for each id.
pub struct DeleteCerts {
    pub ids: Vec<ShortId>,
}

#[async_trait::async_trait]
impl RpcMethod for DeleteCerts {
    type Output = Vec<DeleteCertResult>;
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        if self.ids.len() > MAX_DELETE_CERTS {
            return Err(Error::TooManyCertificates {
                max: MAX_DELETE_CERTS,
            });
        }
        let mut seen = HashSet::new();
        let mut results = Vec::new();
        for id in self.ids.into_iter().filter(|id| seen.insert(*id)) {
            let status = delete_status(id, remove_cert(state, id).await);
            results.push(DeleteCertResult { id, status });
        }
        if results
            .iter()
            .any(|r| r.status == DeleteCertStatus::Deleted)
        {
            state.update_certs().await;
            state.reload_proxies().await;
        }
        Ok(results)
    }

    fn audit(&self) -> Option<AuditRecord> {
        let ids = self.ids.iter().map(ToString::to_string);
        let record = AuditRecord::new(AuditAction::DeleteCert);
        Some(record.summary(ids.collect::<Vec<_>>().join(", ")))
    }
}

/// Removes a certificate from the storage and the state. The caller updates the certificates of
/// the ports and the proxies.
async fn remove_cert(state: &mut ServerState, id: ShortId) -> Result<(), Error> {
    let cert = state
        .certs
        .get(id)
        .ok_or_else(|| Error::IdNotFound { id: id.to_string() })?;
    if cert.source.is_some() {
        return Err(Error::CertificateReadOnly { id });
    }
    ensure_unused(state, id)?;
    state.storage.delete_cert(id).await?;
    state.certs.delete(id)?;
    Ok(())
}

fn delete_status(id: ShortId, result: Result<(), Error>) -> DeleteCertStatus {
    match result {
        Ok(()) => DeleteCertStatus::Deleted,
        Err(Error::IdNotFound { .. }) => DeleteCertStatus::NotFound,
        Err(Error::CertificateInUse { .. }) => DeleteCertStatus::InUse,
        Err(Error::CertificateReadOnly { .. }) => DeleteCertStatus::ReadOnly,
        Err(err) => {
            error!(%id, "failed to delete the certificate: {err}");
            DeleteCertStatus::Failed
        }
    }
}

/// Rejects the deletion of a certificate that a port, a proxy or a discovery provider uses.
fn ensure_unused(state: &ServerState, id: ShortId) -> Result<(), Error> {
    let discovery = &state.config().discovery;
    let provider_certs = [discovery.docker.client_cert, discovery.etcd.client_cert];
    if provider_certs.contains(&Some(id)) {
        return Err(Error::CertificateInUse { id });
    }
    let used_by_port = state.ports.entries().any(|entry| {
        entry
            .port
            .opts
            .tls_termination
            .as_ref()
            .is_some_and(|tls| tls.client_ca_certs.contains(&id))
    });
    let used_by_proxy = state
        .proxies
        .entries()
        .any(|entry| entry.proxy.kind.client_cert() == Some(id));
    if used_by_port || used_by_proxy {
        return Err(Error::CertificateInUse { id });
    }
    Ok(())
}

pub struct DownloadCert {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for DownloadCert {
    type Output = Bytes;
    /// The archive holds the private key.
    const PERMISSION: Permission = Permission::Edit;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let cert = state.certs.get(self.id).ok_or_else(|| Error::IdNotFound {
            id: self.id.to_string(),
        })?;
        cert_to_tar_gz(cert).map_err(|err| {
            error!(id = %self.id, "failed to create the certificate archive: {err:#}");
            Error::FailedToCreateCertificateArchive
        })
    }
}

fn cert_to_tar_gz(cert: &Cert) -> anyhow::Result<Bytes> {
    let mut buf = Vec::<u8>::new();

    {
        let enc = GzEncoder::new(&mut buf, Compression::default());
        let mut tar = tar::Builder::new(enc);

        let mut chain = cert.pem_chain.as_slice();

        let mtime = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut header = Header::new_old();
        header.set_size(chain.len() as _);
        header.set_mtime(mtime);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "chain.pem", &mut chain)?;

        if let Some(key) = &cert.pem_key {
            let mut key = key.as_slice();
            let mut header = Header::new_old();
            header.set_size(key.len() as _);
            header.set_mtime(mtime);
            header.set_mode(0o600);
            header.set_cksum();
            tar.append_data(&mut header, "key.pem", &mut key)?;
        }

        // Dropping the encoder writes the gzip trailer without reporting a write error.
        tar.into_inner()?.finish()?;
    }

    Ok(buf.into())
}

pub struct GetCertCoverage {
    pub domains: Vec<String>,
}

#[async_trait::async_trait]
impl RpcMethod for GetCertCoverage {
    type Output = Vec<DomainCoverageInfo>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let default_target = state
            .config()
            .public_url
            .as_ref()
            .and_then(|u| url::Url::parse(u.as_str()).ok())
            .and_then(|u| u.host_str().map(ToString::to_string))
            .unwrap_or_else(|| "localhost".to_string());
        Ok(check_domains_coverage(
            &state.certs,
            &self.domains,
            &default_target,
        ))
    }
}

pub fn check_domains_coverage(
    certs: &CertList,
    domains: &[String],
    default_target: &str,
) -> Vec<DomainCoverageInfo> {
    domains
        .iter()
        .map(|domain| evaluate_domain_coverage(certs, domain, default_target))
        .collect()
}

fn find_active_wildcard_cert<'a>(
    certs: &'a CertList,
    domain: &str,
) -> Option<(&'a Arc<Cert>, String)> {
    for cert in certs.iter() {
        if cert.kind != CertKind::Server || cert.key.is_none() || !cert.is_valid() {
            continue;
        }
        for san in &cert.san {
            if let SubjectName::WildcardDnsName(root) = san
                && san.test(domain)
            {
                return Some((cert, format!("*.{root}")));
            }
        }
    }
    None
}

fn find_active_dedicated_cert<'a>(
    certs: &'a CertList,
    subject: &SubjectName,
) -> Option<&'a Arc<Cert>> {
    certs.iter().find(|cert| {
        cert.kind == CertKind::Server
            && cert.key.is_some()
            && cert.is_valid()
            && cert.has_subject_name(subject)
    })
}

fn find_expired_cert<'a>(certs: &'a CertList, subject: &SubjectName) -> Option<&'a Arc<Cert>> {
    certs.iter().find(|cert| {
        cert.kind == CertKind::Server
            && cert.key.is_some()
            && cert.has_subject_name(subject)
            && !cert.is_valid()
    })
}

fn evaluate_domain_coverage(
    certs: &CertList,
    domain: &str,
    default_target: &str,
) -> DomainCoverageInfo {
    let clean_domain = domain.trim().trim_end_matches('.');
    let Ok(subject) = clean_domain.parse::<SubjectName>() else {
        return DomainCoverageInfo::new(clean_domain, DomainSslStatus::NoCertificate);
    };

    if let Some((wildcard_cert, pattern)) = find_active_wildcard_cert(certs, clean_domain) {
        return DomainCoverageInfo::new(
            clean_domain,
            DomainSslStatus::CoveredByWildcard {
                wildcard_cert_id: wildcard_cert.id,
                pattern,
            },
        );
    }

    if let Some(dedicated_cert) = find_active_dedicated_cert(certs, &subject) {
        return DomainCoverageInfo::new(
            clean_domain,
            DomainSslStatus::DedicatedCert {
                cert_id: dedicated_cert.id,
            },
        );
    }

    if let Some(expired_cert) = find_expired_cert(certs, &subject) {
        return DomainCoverageInfo::new(
            clean_domain,
            DomainSslStatus::Expired {
                cert_id: expired_cert.id,
            },
        );
    }

    let status = if extract_root_domain(clean_domain).is_some() {
        DomainSslStatus::CnameOrExternal {
            suggested_cname_target: default_target.to_string(),
        }
    } else {
        DomainSslStatus::NoCertificate
    };
    DomainCoverageInfo::new(clean_domain, status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;

    #[test]
    fn the_archive_holds_the_chain_and_a_private_key_that_only_the_owner_reads() {
        let cert = Cert::new_ca().unwrap();
        let archive = cert_to_tar_gz(&cert).unwrap();
        let mut tar = tar::Archive::new(GzDecoder::new(archive.as_ref()));
        let files = tar
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let path = entry.path().unwrap().display().to_string();
                (path, entry.header().mode().unwrap())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            files,
            [
                ("chain.pem".to_string(), 0o644),
                ("key.pem".to_string(), 0o600)
            ]
        );
    }

    #[tokio::test]
    async fn test_check_domains_coverage() {
        let root = Cert::new_ca().unwrap();
        let wildcard =
            Arc::new(Cert::new_self_signed(&["*.example.com".parse().unwrap()], &root).unwrap());
        let dedicated =
            Arc::new(Cert::new_self_signed(&["myapi.other.com".parse().unwrap()], &root).unwrap());

        let certs = CertList::new([wildcard.clone(), dedicated.clone()]).await;

        let results = check_domains_coverage(
            &certs,
            &[
                "sub.example.com".to_string(),
                "myapi.other.com".to_string(),
                "portal.custom.org".to_string(),
            ],
            "app.example.com",
        );

        assert_eq!(results.len(), 3);
        assert!(results[0].is_wildcard_shared);
        assert_eq!(
            results[0].status,
            DomainSslStatus::CoveredByWildcard {
                wildcard_cert_id: wildcard.id,
                pattern: "*.example.com".to_string(),
            }
        );
        assert_eq!(
            results[1].status,
            DomainSslStatus::DedicatedCert {
                cert_id: dedicated.id,
            }
        );
        assert_eq!(
            results[2].status,
            DomainSslStatus::CnameOrExternal {
                suggested_cname_target: "app.example.com".to_string(),
            }
        );
    }
}
