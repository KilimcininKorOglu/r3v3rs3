//! A root certificate with the server and client certificates it signs, plus the test helpers that
//! every client authentication test needs.

use axum::{Router, http::HeaderMap, routing::get};
use r3v3rs3::certs::Cert;
use r3v3rs3_api::{
    id::ShortId,
    multiaddr::Multiaddr,
    port::{PortEntry, PortOptions},
    tls::{ClientAuthMode, TlsTermination},
};
use reqwest::Identity;
use std::{collections::HashMap, future::IntoFuture, sync::Arc};

use super::{TestPort, alloc_tcp_port, port_entry};

/// A root certificate with a server certificate and a client certificate that it signs.
pub struct Pki {
    pub root: Arc<Cert>,
    pub server: Arc<Cert>,
    pub client: Arc<Cert>,
}

impl Pki {
    pub fn new() -> Self {
        let root = Arc::new(Cert::new_ca().unwrap());
        let server =
            Arc::new(Cert::new_self_signed(&["localhost".parse().unwrap()], &root).unwrap());
        let client =
            Arc::new(Cert::new_client(&["client.example.com".parse().unwrap()], &root).unwrap());
        Self {
            root,
            server,
            client,
        }
    }

    pub fn certs(&self) -> HashMap<ShortId, Arc<Cert>> {
        [&self.root, &self.server, &self.client]
            .into_iter()
            .map(|cert| (cert.id, cert.clone()))
            .collect()
    }

    /// Returns a client that trusts the root certificate and sends the client certificate.
    pub fn client(&self, identity: Option<&Cert>) -> anyhow::Result<reqwest::Client> {
        let builder = reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(&self.root.pem_chain)?);
        let builder = match identity {
            Some(cert) => builder.identity(identity_of(cert)?),
            None => builder,
        };
        Ok(builder.build()?)
    }
}

impl Default for Pki {
    fn default() -> Self {
        Self::new()
    }
}

pub fn identity_of(cert: &Cert) -> anyhow::Result<Identity> {
    let key = cert
        .pem_key
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("certificate has no private key"))?;
    Ok(Identity::from_pem(
        &[key.as_slice(), &cert.pem_chain].concat(),
    )?)
}

/// A client certificate that another root certificate signs.
pub fn foreign_client_cert() -> Cert {
    let root = Cert::new_ca().unwrap();
    Cert::new_client(&["client.example.com".parse().unwrap()], &root).unwrap()
}

/// A port entry with TLS termination and the client authentication mode.
pub fn tls_port(
    id: &str,
    listen: Multiaddr,
    mode: ClientAuthMode,
    roots: Vec<ShortId>,
) -> PortEntry {
    let mut entry = port_entry(id, listen);
    entry.port.opts = PortOptions {
        tls_termination: Some(TlsTermination {
            server_names: vec!["localhost".into()],
            client_auth: mode,
            client_ca_certs: roots,
        }),
        proxy_protocol: None,
    };
    entry
}

/// Starts a plain HTTP upstream server that returns the client certificate headers.
pub async fn start_echo_upstream() -> anyhow::Result<TestPort> {
    async fn echo(headers: HeaderMap) -> String {
        let value = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("-")
                .to_string()
        };
        format!(
            "{}|{}",
            value("x-client-cert"),
            value("x-client-cert-fingerprint")
        )
    }
    let port = alloc_tcp_port().await?;
    let listener = tokio::net::TcpListener::bind(port.socket_addr()).await?;
    tokio::spawn(axum::serve(listener, Router::new().route("/", get(echo))).into_future());
    Ok(port)
}
