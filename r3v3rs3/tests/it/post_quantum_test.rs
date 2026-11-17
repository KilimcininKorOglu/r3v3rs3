//! The TLS key exchange of the server binary. `main` installs the aws-lc-rs provider, which offers
//! the post-quantum X25519MLKEM768 hybrid, so a TLS port of the running binary must agree on it
//! with a client that offers only this group.

use crate::common;
use common::{alloc_tcp_port, port_entry};
use r3v3rs3::certs::Cert;
use r3v3rs3::config::sqlite::SqliteStorage;
use r3v3rs3::config::storage::Storage as _;
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::port::UpstreamServer;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry, ProxyKind, TcpProxy};
use r3v3rs3_api::tls::TlsTermination;
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::crypto::aws_lc_rs;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, NamedGroup, RootCertStore};

/// A TCP proxy of the port to an address that nothing serves: the handshake ends at the port.
fn tcp_proxy() -> anyhow::Result<Proxy> {
    Ok(Proxy {
        ports: vec!["pq".parse()?],
        kind: ProxyKind::Tcp(TcpProxy {
            client_cert: None,
            connect_timeout: r3v3rs3_api::upstream::DEFAULT_CONNECT_TIMEOUT,
            load_balancing: Default::default(),
            health_check: Default::default(),
            circuit_breaker: Default::default(),
            proxy_protocol: None,
            upstream_servers: vec![UpstreamServer::new("/ip4/127.0.0.1/tcp/1".parse()?)],
        }),
        ..Default::default()
    })
}

/// Writes a TLS port for `localhost` with its certificate into the config directory.
async fn write_config(
    dir: &std::path::Path,
    port: &common::TestPort,
    cert: &Cert,
) -> anyhow::Result<()> {
    let storage = SqliteStorage::open(dir).await?;
    storage.save_app_config(&AppConfig::default()).await?;
    let mut entry = port_entry("pq", port.multiaddr_tls());
    entry.port.opts.tls_termination = Some(TlsTermination {
        server_names: vec!["localhost".into()],
        ..Default::default()
    });
    storage.save_ports(&[entry]).await?;
    storage.save_cert(cert).await?;
    storage
        .save_proxies(&[ProxyEntry {
            id: "pq-proxy".parse()?,
            source: None,
            status: None,
            proxy: tcp_proxy()?,
        }])
        .await?;
    Ok(())
}

/// A client that trusts only `root` and offers only the X25519MLKEM768 group.
fn post_quantum_client(root: &Cert) -> anyhow::Result<TlsConnector> {
    let mut roots = RootCertStore::empty();
    for der in CertificateDer::pem_slice_iter(&root.pem_chain) {
        roots.add(der?)?;
    }
    let mut provider = aws_lc_rs::default_provider();
    provider.kx_groups = vec![aws_lc_rs::kx_group::X25519MLKEM768];
    let config = ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

/// Connects until the server listens, then completes the handshake and returns the group.
async fn handshake_group(
    port: &common::TestPort,
    connector: &TlsConnector,
) -> anyhow::Result<Option<NamedGroup>> {
    let mut last = None;
    for _ in 0..100 {
        match tokio::net::TcpStream::connect(port.socket_addr()).await {
            Ok(stream) => {
                let name = ServerName::try_from("localhost")?;
                let tls = connector.connect(name, stream).await?;
                return Ok(tls
                    .get_ref()
                    .1
                    .negotiated_key_exchange_group()
                    .map(|group| group.name()));
            }
            Err(err) => last = Some(err),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("the server does not listen: {last:?}")
}

/// Creates the config directory with a certificate for `localhost` and returns its root.
async fn prepare(dir: &std::path::Path, port: &common::TestPort) -> anyhow::Result<Cert> {
    let root = Cert::new_ca()?;
    let cert = Cert::new_self_signed(&["localhost".parse()?], &root)?;
    std::fs::create_dir_all(dir)?;
    write_config(dir, port, &cert).await?;
    Ok(root)
}

/// Starts the server binary without the panel on the config directory.
fn start_server(dir: &std::path::Path) -> anyhow::Result<std::process::Child> {
    Ok(std::process::Command::new(env!("CARGO_BIN_EXE_r3-server"))
        .args(["start", "--no-webui", "--config-dir"])
        .arg(dir)
        .arg("--log-dir")
        .arg(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?)
}

/// Returns the exit status if the server exited on its own, then stops it.
async fn stop_server(
    mut server: std::process::Child,
) -> anyhow::Result<Option<std::process::ExitStatus>> {
    // Nothing asked for an upgrade, so the started process still serves: a process that hands
    // its listeners over on its own leaves orphans that keep the port.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let exited = server.try_wait()?;
    server.kill().ok();
    server.wait()?;
    Ok(exited)
}

#[tokio::test]
async fn a_tls_handshake_negotiates_the_post_quantum_group() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let dir = std::env::temp_dir().join(format!("r3v3rs3-pq-{}", std::process::id()));
    let root = prepare(&dir, &port).await?;

    let server = start_server(&dir)?;
    let group = handshake_group(&port, &post_quantum_client(&root)?).await;
    let exited = stop_server(server).await?;
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(exited, None, "the server process exited without a request");
    assert_eq!(group?, Some(NamedGroup::X25519MLKEM768));
    Ok(())
}
