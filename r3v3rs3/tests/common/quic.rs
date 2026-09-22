//! A QUIC client that speaks HTTP/3, so a test can drive a `/quic/https` port of the server.
//!
//! The quinn of `h3_quinn` is the one the server uses, so the client and the server always share
//! one quinn version.

use bytes::Buf;
use h3_quinn::quinn::{self, crypto::rustls::QuicClientConfig};
use hyper::http::{Request, StatusCode};
use r3v3rs3::certs::Cert;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

/// Builds a client endpoint that trusts `root`. `identity` sends a client certificate.
pub fn quic_endpoint(root: &Cert, identity: Option<&Cert>) -> anyhow::Result<quinn::Endpoint> {
    let mut roots = RootCertStore::empty();
    for der in root.certificates()? {
        roots.add(der)?;
    }
    let builder = ClientConfig::builder().with_root_certificates(roots);
    let mut tls = match identity {
        Some(cert) => {
            builder.with_client_auth_cert(cert.certificates()?, cert.private_key_der()?)?
        }
        None => builder.with_no_client_auth(),
    };
    tls.alpn_protocols = vec![b"h3".to_vec()];

    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(tls)?,
    )));
    Ok(endpoint)
}

/// Sends one HTTP/3 GET request and returns the status and the body.
pub async fn h3_get(
    endpoint: &quinn::Endpoint,
    addr: SocketAddr,
    server_name: &str,
    url: &str,
    headers: &[(&str, &str)],
) -> anyhow::Result<(StatusCode, String)> {
    let conn = endpoint.connect(addr, server_name)?.await?;
    let (mut driver, mut send_request) = h3::client::new(h3_quinn::Connection::new(conn)).await?;
    // The driver moves the connection forward while the request runs.
    let driver =
        tokio::spawn(async move { std::future::poll_fn(|cx| driver.poll_close(cx)).await });

    let mut builder = Request::get(url);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let mut stream = send_request.send_request(builder.body(())?).await?;
    stream.finish().await?;

    let response = stream.recv_response().await?;
    let mut body = Vec::new();
    while let Some(mut chunk) = stream.recv_data().await? {
        body.extend_from_slice(chunk.copy_to_bytes(chunk.remaining()).as_ref());
    }
    drop(send_request);
    driver.abort();
    Ok((response.status(), String::from_utf8(body)?))
}

/// Waits until the QUIC port of the server accepts a connection. `wait_for_listener` only knows
/// TCP, so this retries a real handshake instead.
pub async fn wait_for_quic(
    endpoint: &quinn::Endpoint,
    addr: SocketAddr,
    server_name: &str,
) -> anyhow::Result<()> {
    for _ in 0..100 {
        if endpoint.connect(addr, server_name)?.await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("the quic port of {addr} did not accept a connection")
}
