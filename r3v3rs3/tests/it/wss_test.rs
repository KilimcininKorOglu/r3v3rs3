use axum::{
    Router,
    extract::{WebSocketUpgrade, ws::WebSocket},
    response::IntoResponse,
    routing::any,
};
use axum_server::tls_rustls::RustlsConfig;
use futures::{SinkExt, StreamExt};
use hyper::Uri;
use r3v3rs3::certs::Cert;
use r3v3rs3_api::{
    port::{Port, PortEntry, PortOptions},
    proxy::{HttpProxy, Proxy, ProxyEntry, ProxyKind, Route},
    tls::TlsTermination,
};
use std::sync::Arc;
use tokio_rustls::rustls::{RootCertStore, client::ClientConfig};
use tokio_tungstenite::{
    Connector, connect_async_tls_with_config,
    tungstenite::{Error as WsError, Message},
};

use crate::common;
use common::{TestPort, TestStorage, alloc_tcp_port, with_server};

/// Starts a TLS WebSocket echo server on `port` with `cert`.
async fn spawn_upstream(port: &TestPort, cert: &Cert) -> anyhow::Result<()> {
    let key = cert
        .pem_key
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the upstream certificate has no key"))?;
    let config = RustlsConfig::from_pem(cert.pem_chain.to_vec(), key.to_vec()).await?;
    let app = Router::new().route("/ws", any(ws_handler));
    tokio::spawn(
        axum_server::bind_rustls(port.socket_addr(), config).serve(app.into_make_service()),
    );
    Ok(())
}

/// A TLS port on `proxy_port` that routes `localhost` to the TLS server on `listen_port`. The
/// proxy holds `certs`, so it trusts only the root certificates among them.
fn storage(listen_port: &TestPort, proxy_port: &TestPort, certs: &[Arc<Cert>]) -> TestStorage {
    TestStorage::builder()
        .ports(vec![PortEntry {
            id: "test".parse().unwrap(),
            port: Port {
                active: true,
                name: String::new(),
                listen: proxy_port.multiaddr_https(),
                opts: PortOptions {
                    tls_termination: Some(TlsTermination {
                        server_names: vec!["localhost".into()],
                        ..Default::default()
                    }),
                    proxy_protocol: None,
                    http2_limits: None,
                    http3_limits: None,
                    stream_priority: None,
                },
            },
            status: None,
        }])
        .proxies(vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            status: None,
            proxy: Proxy {
                ports: vec!["test".parse().unwrap()],
                kind: ProxyKind::Http(Box::new(HttpProxy {
                    vhosts: vec!["localhost".parse().unwrap()],
                    routes: vec![Route {
                        servers: vec![r3v3rs3_api::proxy::Server::new(
                            listen_port.https_url("/").try_into().unwrap(),
                        )],
                        ..Default::default()
                    }],
                    upgrade_insecure: false,
                    client_ip: Default::default(),
                    ip_filter: Default::default(),
                    rate_limit: Default::default(),
                    concurrency_limit: Default::default(),
                    auth: Default::default(),
                    headers: Default::default(),
                    compression: Default::default(),
                    cache: Default::default(),
                    h2c: false,
                    timeouts: Default::default(),
                    load_balancing: Default::default(),
                    health_check: Default::default(),
                    circuit_breaker: Default::default(),
                    retry: Default::default(),
                    sticky: Default::default(),
                    access_list: None,
                    max_body_size: 0,
                    redirects: Vec::new(),
                    client_cert: None,
                    maintenance: Default::default(),
                    error_page_theme: None,
                    error_pages: Vec::new(),
                    bot_blocker: false,
                    grpc_web: false,
                    preserve_host: false,
                    stream_priority: Default::default(),
                    hide_branding: false,
                })),
                ..Default::default()
            },
        }])
        .certs(certs.iter().map(|cert| (cert.id, cert.clone())).collect())
        .build()
}

/// A client that trusts `root`, the CA of the certificate that the proxy presents.
fn client(root: &Cert) -> anyhow::Result<Connector> {
    let mut roots = RootCertStore::empty();
    for cert in root.certified_key()?.cert {
        roots.add(cert)?;
    }
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}

fn wss_url(proxy_port: &TestPort) -> anyhow::Result<Uri> {
    let port = proxy_port.socket_addr().port();
    Ok(Uri::try_from(format!("wss://localhost:{port}/ws"))?)
}

/// A running TLS upstream, the proxy configuration that routes to it, and a client of the proxy.
struct Setup {
    proxy_port: TestPort,
    config: TestStorage,
    connector: Connector,
}

/// The certificate of the upstream: from the CA that the proxy trusts, or from a stranger CA.
fn upstream_cert(root: &Cert, trusted: bool) -> anyhow::Result<Cert> {
    let stranger;
    let ca = if trusted {
        root
    } else {
        stranger = Cert::new_ca()?;
        &stranger
    };
    Ok(Cert::new_self_signed(&["localhost".parse()?], ca)?)
}

/// Starts the upstream and builds the proxy configuration. The proxy holds only `root` and its
/// own certificate.
async fn setup(trusted_upstream: bool) -> anyhow::Result<Setup> {
    let listen_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;
    let root = Arc::new(Cert::new_ca()?);
    let proxy_cert = Arc::new(Cert::new_self_signed(&["localhost".parse()?], &root)?);
    spawn_upstream(&listen_port, &upstream_cert(&root, trusted_upstream)?).await?;
    let config = storage(&listen_port, &proxy_port, &[root.clone(), proxy_cert]);
    let connector = client(&root)?;
    Ok(Setup {
        proxy_port,
        config,
        connector,
    })
}

/// Sends one message over the proxy and requires the echo.
async fn echo_once(proxy_port: TestPort, connector: Connector) -> anyhow::Result<()> {
    let url = wss_url(&proxy_port)?;
    let (mut ws_stream, _) =
        connect_async_tls_with_config(url, None, false, Some(connector)).await?;
    ws_stream
        .send(Message::Text("Hello, server!".to_string().into()))
        .await?;
    let message = ws_stream
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("the socket closed"))??;
    assert_eq!("Hello, server!", message.into_text()?.as_str());
    Ok(())
}

/// Requires that the proxy refuses the WebSocket with 526.
async fn refused(proxy_port: TestPort, connector: Connector) -> anyhow::Result<()> {
    let url = wss_url(&proxy_port)?;
    match connect_async_tls_with_config(url, None, false, Some(connector)).await {
        Err(WsError::Http(response)) => assert_eq!(response.status(), 526),
        Err(err) => anyhow::bail!("expected a 526 answer, got {err}"),
        Ok(_) => anyhow::bail!("the proxy opened a WebSocket to an untrusted upstream"),
    }
    Ok(())
}

/// A WebSocket message goes through the TLS port to a TLS upstream whose CA the proxy trusts.
#[tokio::test]
async fn wss_proxy() -> anyhow::Result<()> {
    let Setup {
        proxy_port,
        config,
        connector,
    } = setup(true).await?;
    with_server(config, |_| echo_once(proxy_port, connector)).await
}

/// A TLS upstream whose certificate comes from a CA that the proxy does not hold can be anyone on
/// the path, so the proxy must not open the WebSocket to it. The client gets 526 (Invalid SSL
/// Certificate), the gateway code that names a refused origin certificate.
#[tokio::test]
async fn wss_refuses_an_upstream_with_an_untrusted_certificate() -> anyhow::Result<()> {
    let Setup {
        proxy_port,
        config,
        connector,
    } = setup(false).await?;
    with_server(config, |_| refused(proxy_port, connector)).await
}

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    while let Some(Ok(msg)) = socket.recv().await {
        if socket.send(msg).await.is_err() {
            return;
        }
    }
}
