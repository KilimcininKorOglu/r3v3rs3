use axum_server::tls_rustls::RustlsConfig;
use r3v3rs3::certs::Cert;
use r3v3rs3_api::proxy::HttpProxy;
use reqwest::StatusCode;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::sync::Arc;

mod common;
use common::{
    TestPort, TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route,
    with_server,
};

/// The host name that the tests send as the client host.
const CLIENT_HOST: &str = "app.test";

/// Sends a request for `localhost` through a proxy and checks that the upstream server receives
/// the `Host` header that `expected_host` returns for its address.
async fn upstream_host(
    preserve_host: bool,
    expected_host: fn(&str) -> String,
) -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/")
        .match_header("host", expected_host(&server.host_with_port()).as_str())
        .with_status(200)
        .with_body("ok")
        .create_async()
        .await;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &server.url(), None)],
        preserve_host,
        ..Default::default()
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("hst-test", "http", proxy)])
        .build();

    with_server(config, |_| async move {
        let res = reqwest::Client::new()
            .get(port.http_url("/"))
            .header("Host", "localhost")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        Ok(())
    })
    .await?;
    mock.assert_async().await;
    Ok(())
}

#[tokio::test]
async fn the_upstream_receives_the_client_host_when_it_is_kept() -> anyhow::Result<()> {
    upstream_host(true, |_| "localhost".into()).await
}

#[tokio::test]
async fn the_upstream_receives_its_own_address_by_default() -> anyhow::Result<()> {
    upstream_host(false, str::to_string).await
}

/// An upstream app that answers with its name, the HTTP version, the URI authority and the
/// `Host` header of the request.
fn echo_app(name: &'static str) -> axum::Router {
    axum::Router::new().fallback(move |req: axum::extract::Request| async move {
        let authority = req.uri().authority().map(ToString::to_string);
        let host = req
            .headers()
            .get(reqwest::header::HOST)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        format!("{name} {:?} {authority:?} {host:?}", req.version())
    })
}

/// A proxy for `CLIENT_HOST` that keeps the client host, with one route per upstream server.
fn kept_host_proxy(routes: &[(&str, String)], h2c: bool) -> HttpProxy {
    HttpProxy {
        vhosts: vec![CLIENT_HOST.parse().unwrap()],
        routes: routes
            .iter()
            .map(|(path, upstream)| http_route(path, upstream, None))
            .collect(),
        preserve_host: true,
        h2c,
        ..Default::default()
    }
}

/// Sends a request for `CLIENT_HOST` to `path` through the proxy port and returns the answer of
/// the upstream app.
async fn answer(port: &TestPort, path: &str) -> anyhow::Result<String> {
    let res = reqwest::Client::new()
        .get(port.http_url(path))
        .header("Host", CLIENT_HOST)
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    Ok(res.text().await?)
}

#[tokio::test]
async fn an_h2c_upstream_receives_the_client_host_as_its_authority() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let first = alloc_tcp_port().await?;
    let second = alloc_tcp_port().await?;
    tokio::spawn(
        axum_server::bind(first.socket_addr()).serve(echo_app("first").into_make_service()),
    );
    tokio::spawn(
        axum_server::bind(second.socket_addr()).serve(echo_app("second").into_make_service()),
    );

    let routes = [
        ("/first", first.http_url("/").to_string()),
        ("/second", second.http_url("/").to_string()),
    ];
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry(
            "hst-h2c",
            "http",
            kept_host_proxy(&routes, true),
        )])
        .build();

    with_server(config, |_| async move {
        // The authority names the client host, and no second Host header contradicts it. The
        // two servers share the client host, yet each request reaches its own server.
        for name in ["first", "second", "first"] {
            assert_eq!(
                answer(&port, &format!("/{name}")).await?,
                format!(r#"{name} HTTP/2.0 Some("{CLIENT_HOST}") None"#)
            );
        }
        Ok(())
    })
    .await
}

/// A TLS server config for `localhost` with the ALPN protocols.
fn tls_config(cert: &Cert, alpn: &[&[u8]]) -> anyhow::Result<RustlsConfig> {
    let chain = CertificateDer::pem_slice_iter(&cert.pem_chain).collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_slice(cert.pem_key.as_deref().unwrap_or_default())?;
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(chain, key)?;
    config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    Ok(RustlsConfig::from_config(Arc::new(config)))
}

/// Serves `echo_app(name)` over TLS on a new port with the ALPN protocols.
async fn serve_tls(cert: &Cert, name: &'static str, alpn: &[&[u8]]) -> anyhow::Result<TestPort> {
    let port = alloc_tcp_port().await?;
    tokio::spawn(
        axum_server::bind_rustls(port.socket_addr(), tls_config(cert, alpn)?)
            .serve(echo_app(name).into_make_service()),
    );
    Ok(port)
}

#[tokio::test]
async fn a_tls_upstream_receives_the_client_host_over_http2_and_http1() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let root = Arc::new(Cert::new_ca()?);
    let cert = Arc::new(Cert::new_self_signed(&["localhost".parse()?], &root)?);
    let h2 = serve_tls(&cert, "h2", &[b"h2", b"http/1.1"]).await?;
    let http1 = serve_tls(&cert, "http1", &[b"http/1.1"]).await?;

    let routes = [
        ("/h2", h2.https_url("/").to_string()),
        ("/http1", http1.https_url("/").to_string()),
    ];
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry(
            "hst-tls",
            "http",
            kept_host_proxy(&routes, false),
        )])
        .certs(
            [(root.id, root.clone()), (cert.id, cert.clone())]
                .into_iter()
                .collect(),
        )
        .build();

    with_server(config, |_| async move {
        // The TLS connection verifies the certificate of `localhost`, while the request names the
        // client host: as the authority over HTTP/2, as the Host header over HTTP/1.1.
        assert_eq!(
            answer(&port, "/h2").await?,
            format!(r#"h2 HTTP/2.0 Some("{CLIENT_HOST}") None"#)
        );
        assert_eq!(
            answer(&port, "/http1").await?,
            format!(r#"http1 HTTP/1.1 None Some("{CLIENT_HOST}")"#)
        );
        Ok(())
    })
    .await
}
