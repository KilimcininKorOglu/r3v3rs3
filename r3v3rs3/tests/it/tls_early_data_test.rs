use r3v3rs3::certs::Cert;
use r3v3rs3_api::early_data::{
    AntiReplayBuffer, EARLY_DATA_HEADER, EARLY_DATA_VALUE, is_safe_http_method,
};
use r3v3rs3_api::port::{Port, PortEntry, PortOptions};
use r3v3rs3_api::proxy::HttpProxy;
use r3v3rs3_api::tls::TlsTermination;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::common;
use common::{TestStorage, alloc_tcp_port, with_server};

#[test]
fn test_safe_http_methods_per_rfc7231() {
    assert!(is_safe_http_method("GET"));
    assert!(is_safe_http_method("HEAD"));
    assert!(is_safe_http_method("OPTIONS"));
    assert!(is_safe_http_method("TRACE"));

    assert!(!is_safe_http_method("POST"));
    assert!(!is_safe_http_method("PUT"));
    assert!(!is_safe_http_method("DELETE"));
    assert!(!is_safe_http_method("PATCH"));
    assert!(!is_safe_http_method("CONNECT"));
}

#[test]
fn test_anti_replay_buffer_logic() {
    let buffer = AntiReplayBuffer::new(Duration::from_secs(30), 500);
    let client_ip: IpAddr = Ipv4Addr::new(192, 168, 1, 100).into();

    // 1. Initial 0-RTT request is accepted
    assert!(buffer.check_and_record(client_ip, "GET", "/resource"));

    // 2. Immediate replay of exact same request is rejected
    assert!(!buffer.check_and_record(client_ip, "GET", "/resource"));

    // 3. Different URI is accepted
    assert!(buffer.check_and_record(client_ip, "GET", "/resource?query=1"));

    // 4. Different IP is accepted
    let client_ip_2: IpAddr = Ipv4Addr::new(192, 168, 1, 101).into();
    assert!(buffer.check_and_record(client_ip_2, "GET", "/resource"));
}

#[test]
fn test_tls_termination_early_data_serialization() {
    let mut tls = TlsTermination {
        server_names: vec!["example.com".into()],
        early_data: true,
        ..Default::default()
    };

    let serialized = serde_json::to_string(&tls).unwrap();
    assert!(serialized.contains(r#""early_data":true"#));

    let deserialized: TlsTermination = serde_json::from_str(&serialized).unwrap();
    assert!(deserialized.early_data);

    tls.early_data = false;
    let serialized_off = serde_json::to_string(&tls).unwrap();
    assert!(!serialized_off.contains("early_data"));
}

/// An upstream that answers `early-data=<value>` with the `Early-Data` header it received, or
/// `early-data=none`.
async fn echo_early_data_upstream() -> anyhow::Result<url::Url> {
    let app = axum::Router::new().route(
        "/echo",
        axum::routing::any(|headers: axum::http::HeaderMap| async move {
            let value = headers
                .get(EARLY_DATA_HEADER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("none");
            format!("early-data={value}")
        }),
    );
    common::serve_http_upstream(app).await
}

/// An HTTPS port with 0-RTT for `localhost` and a proxy to the upstream.
fn early_data_storage(
    port: &common::TestPort,
    upstream: &url::Url,
    root: &Arc<Cert>,
) -> TestStorage {
    let cert = Arc::new(Cert::new_self_signed(&["localhost".parse().unwrap()], root).unwrap());
    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![common::http_route("/", upstream.as_str(), None)],
        ..Default::default()
    };
    TestStorage::builder()
        .ports(vec![PortEntry {
            id: "tls".parse().unwrap(),
            port: Port {
                active: true,
                name: String::new(),
                listen: port.multiaddr_https(),
                opts: PortOptions {
                    tls_termination: Some(TlsTermination {
                        server_names: vec!["localhost".into()],
                        early_data: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            },
            status: None,
        }])
        .proxies(vec![common::http_proxy_entry("early", "tls", proxy)])
        .certs(
            [(root.id, root.clone()), (cert.id, cert)]
                .into_iter()
                .collect(),
        )
        .build()
}

/// A client that trusts the root, keeps its session tickets and sends early data.
fn early_data_client(root: &Cert) -> anyhow::Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    for der in CertificateDer::pem_slice_iter(&root.pem_chain) {
        roots.add(der?)?;
    }
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.enable_early_data = true;
    Ok(Arc::new(config))
}

/// Sends one HTTP/1.1 request, in early data when `early` is set, and returns the response and
/// whether the server accepted the early data.
async fn send_request(
    config: &Arc<ClientConfig>,
    addr: std::net::SocketAddr,
    request: &str,
    early: bool,
) -> anyhow::Result<(String, bool)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let tcp = tokio::net::TcpStream::connect(addr).await?;
    let connector = TlsConnector::from(config.clone()).early_data(early);
    let mut tls = connector
        .connect(ServerName::try_from("localhost")?, tcp)
        .await?;
    tls.write_all(request.as_bytes()).await?;
    tls.flush().await?;
    let mut response = String::new();
    // A server that never reads the early data leaves the client waiting for the answer.
    tokio::time::timeout(Duration::from_secs(10), tls.read_to_string(&mut response))
        .await
        .map_err(|_| {
            let accepted = tls.get_ref().1.is_early_data_accepted();
            anyhow::anyhow!(
                "no answer to {request:?}, sent in early data: {early}, accepted: {accepted}"
            )
        })??;
    Ok((response, tls.get_ref().1.is_early_data_accepted()))
}

const GET_WITH_HEADER: &str =
    "GET /echo HTTP/1.1\r\nHost: localhost\r\nEarly-Data: 1\r\nConnection: close\r\n\r\n";
/// Without `Connection: close`, so only the server can end the connection.
const GET_KEEP_ALIVE: &str = "GET /echo HTTP/1.1\r\nHost: localhost\r\n\r\n";
const POST: &str =
    "POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// Asserts a 200 answer whose body is the `Early-Data` value that the upstream received.
fn assert_upstream_saw(response: &str, value: &str) {
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response.ends_with(&format!("early-data={value}")),
        "{response}"
    );
}

/// Requests after a full handshake: the header of the client reaches the upstream (RFC 8470
/// section 5.1, an intermediary must not remove it), and a POST is answered. The first request
/// also gives the client the session tickets for 0-RTT.
async fn check_full_handshake(client: &Arc<ClientConfig>, addr: SocketAddr) -> anyhow::Result<()> {
    let (response, _) = send_request(client, addr, GET_WITH_HEADER, false).await?;
    assert_upstream_saw(&response, EARLY_DATA_VALUE);
    let (response, _) = send_request(client, addr, POST, false).await?;
    assert_upstream_saw(&response, "none");
    Ok(())
}

/// Requests in early data: a safe request reaches the upstream with `Early-Data: 1` (section
/// 5.1) and the server then closes the connection, and an unsafe method gets `425 Too Early`
/// (section 5.2).
async fn check_early_data(client: &Arc<ClientConfig>, addr: SocketAddr) -> anyhow::Result<()> {
    // `send_request` reads until the server closes the connection.
    let (response, accepted) = send_request(client, addr, GET_KEEP_ALIVE, true).await?;
    assert!(accepted, "the server refused the early data");
    assert_upstream_saw(&response, EARLY_DATA_VALUE);

    let (response, accepted) = send_request(client, addr, POST, true).await?;
    assert!(accepted, "the server refused the early data");
    assert!(response.starts_with("HTTP/1.1 425"), "{response}");
    Ok(())
}

#[tokio::test]
async fn zero_rtt_requests_follow_rfc_8470() -> anyhow::Result<()> {
    let upstream = echo_early_data_upstream().await?;
    let port = alloc_tcp_port().await?;
    let root = Arc::new(Cert::new_ca()?);
    let storage = early_data_storage(&port, &upstream, &root);
    let client = early_data_client(&root)?;
    with_server(storage, |_| async move {
        let addr = port.socket_addr();
        check_full_handshake(&client, addr).await?;
        check_early_data(&client, addr).await?;
        // A resumed connection whose early data extension carried no bytes is no early request.
        let (response, _) = send_request(&client, addr, POST, false).await?;
        assert_upstream_saw(&response, "none");
        Ok(())
    })
    .await
}
