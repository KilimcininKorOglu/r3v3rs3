use super::RpcMethod;
use crate::{accounts::Permission, audit::AuditRecord, certs::Cert, server::state::ServerState};
use flate2::{Compression, write::GzEncoder};
use hyper::body::Bytes;
use r3v3rs3_api::{
    audit::AuditAction,
    cert::{CertInfo, DeleteCertResult, DeleteCertStatus, MAX_DELETE_CERTS},
    error::Error,
    id::ShortId,
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
    let provider_certs = [
        discovery.docker.client_cert,
        discovery.consul.client_cert,
        discovery.etcd.client_cert,
    ];
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
}
