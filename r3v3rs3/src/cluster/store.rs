//! The connection to the store of the cluster.

use crate::kv::consul::ConsulClient;
use crate::kv::etcd::EtcdClient;
use crate::kv::http::ApiClient;
use crate::kv::{ConsulStore, KvStore};
use anyhow::{anyhow, Context as _};
use r3v3rs3_api::cluster::{ClusterBackend, ClusterConfig, ClusterTlsConfig};
use r3v3rs3_api::discovery::Endpoint;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

/// The shortest session TTL that Consul accepts.
const MIN_CONSUL_LOCK_TTL: Duration = Duration::from_secs(10);

pub fn validate(config: &ClusterConfig) -> anyhow::Result<()> {
    if config.endpoints.is_empty() {
        anyhow::bail!("cluster.endpoints is empty");
    }
    if config.prefix.trim_matches('/').is_empty() {
        anyhow::bail!("cluster.prefix is empty");
    }
    // The presence key, the lock value, the challenge ack and the rate limit counts of a node use
    // its name, so every node needs its own name.
    if config.node_name.trim().is_empty() {
        anyhow::bail!("cluster.node_name is empty");
    }
    if config.encryption_key_files.is_empty() {
        anyhow::bail!("cluster.encryption_key_files is empty");
    }
    if config.tls.cert_file.is_some() != config.tls.key_file.is_some() {
        anyhow::bail!("cluster.tls needs both cert_file and key_file");
    }
    let min_ttl = match config.backend {
        ClusterBackend::Etcd => Duration::from_secs(1),
        ClusterBackend::Consul => MIN_CONSUL_LOCK_TTL,
    };
    if config.lock_ttl < min_ttl {
        anyhow::bail!("cluster.lock_ttl is shorter than {}s", min_ttl.as_secs());
    }
    Ok(())
}

/// Connects to the store. etcd authenticates before the call returns.
pub async fn connect(config: &ClusterConfig) -> anyhow::Result<Arc<dyn KvStore>> {
    validate(config)?;
    let endpoints = config
        .endpoints
        .iter()
        .map(|endpoint| {
            endpoint
                .parse::<Endpoint>()
                .map_err(|err| anyhow!("invalid cluster endpoint {endpoint}: {err}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let client = ApiClient::new(endpoints, Arc::new(tls_config(&config.tls).await?));
    let store: Arc<dyn KvStore> = match config.backend {
        ClusterBackend::Etcd => {
            let store = EtcdClient::new(client, &config.username, config.password.as_deref());
            store.authenticate().await?;
            Arc::new(store)
        }
        ClusterBackend::Consul => {
            let client = ConsulClient::new(client, config.token.as_deref());
            Arc::new(ConsulStore::new(client, &config.datacenter))
        }
    };
    Ok(store)
}

async fn tls_config(tls: &ClusterTlsConfig) -> anyhow::Result<ClientConfig> {
    let roots = root_certs(tls.ca_file.as_deref()).await?;
    let builder = ClientConfig::builder().with_root_certificates(roots);
    let (Some(cert_file), Some(key_file)) = (&tls.cert_file, &tls.key_file) else {
        return Ok(builder.with_no_client_auth());
    };
    let chain = read_certs(cert_file).await?;
    let key = read_key(key_file).await?;
    builder
        .with_client_auth_cert(chain, key)
        .context("invalid cluster client certificate")
}

/// The certificates of `ca_file`, or the system root certificates.
async fn root_certs(ca_file: Option<&Path>) -> anyhow::Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    let Some(ca_file) = ca_file else {
        let native = tokio::task::spawn_blocking(rustls_native_certs::load_native_certs).await?;
        roots.add_parsable_certificates(native.certs);
        return Ok(roots);
    };
    for cert in read_certs(ca_file).await? {
        roots
            .add(cert)
            .with_context(|| format!("invalid CA certificate in {}", ca_file.display()))?;
    }
    Ok(roots)
}

async fn read_certs(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let certs = rustls_pemfile::certs(&mut read_pem(path).await?.as_slice())
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("invalid certificate in {}", path.display()))?;
    anyhow::ensure!(!certs.is_empty(), "{} has no certificate", path.display());
    Ok(certs)
}

async fn read_key(path: &Path) -> anyhow::Result<PrivateKeyDer<'static>> {
    rustls_pemfile::private_key(&mut read_pem(path).await?.as_slice())
        .with_context(|| format!("invalid private key in {}", path.display()))?
        .with_context(|| format!("{} has no private key", path.display()))
}

async fn read_pem(path: &Path) -> anyhow::Result<Vec<u8>> {
    tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ClusterConfig {
        ClusterConfig {
            enabled: true,
            endpoints: vec!["http://127.0.0.1:2379".into()],
            node_name: "node-a".into(),
            encryption_key_files: vec!["/etc/r3v3rs3/cluster.key".into()],
            ..Default::default()
        }
    }

    #[test]
    fn validate_rejects_an_incomplete_config() {
        assert!(validate(&config()).is_ok());

        let errors = [
            ClusterConfig {
                endpoints: Vec::new(),
                ..config()
            },
            ClusterConfig {
                prefix: "/".into(),
                ..config()
            },
            ClusterConfig {
                node_name: " ".into(),
                ..config()
            },
            ClusterConfig {
                encryption_key_files: Vec::new(),
                ..config()
            },
            ClusterConfig {
                tls: ClusterTlsConfig {
                    cert_file: Some("/etc/r3v3rs3/client.pem".into()),
                    ..Default::default()
                },
                ..config()
            },
            ClusterConfig {
                backend: ClusterBackend::Consul,
                lock_ttl: Duration::from_secs(5),
                ..config()
            },
        ];
        for config in errors {
            assert!(validate(&config).is_err(), "{config:?}");
        }
    }
}
