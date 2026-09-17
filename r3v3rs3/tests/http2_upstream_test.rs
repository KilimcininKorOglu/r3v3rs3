use axum::{Router, extract::Request, routing::get};
use axum_server::tls_rustls::RustlsConfig;
use r3v3rs3::certs::Cert;
use r3v3rs3_api::proxy::HttpProxy;
use reqwest::header::HOST;
use std::sync::Arc;

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

async fn version(req: Request) -> String {
    format!("{:?}", req.version())
}

fn proxy(vhost: &str, upstream: &str, h2c: bool) -> HttpProxy {
    HttpProxy {
        vhosts: vec![vhost.parse().unwrap()],
        routes: vec![http_route("/", upstream, None)],
        upgrade_insecure: false,
        h2c,
        ..Default::default()
    }
}

#[tokio::test]
async fn upstream_uses_http2_with_alpn_and_h2c() -> anyhow::Result<()> {
    let proxy_port = alloc_tcp_port().await?;
    let tls_port = alloc_tcp_port().await?;
    let plain_port = alloc_tcp_port().await?;

    let root = Arc::new(Cert::new_ca().unwrap());
    let cert = Arc::new(Cert::new_self_signed(&["localhost".parse().unwrap()], &root).unwrap());
    let tls = RustlsConfig::from_pem(
        cert.pem_chain.to_vec(),
        cert.pem_key.as_ref().unwrap().to_vec(),
    )
    .await
    .unwrap();

    let app = Router::new().route("/version", get(version));
    tokio::spawn(
        axum_server::bind_rustls(tls_port.socket_addr(), tls)
            .serve(app.clone().into_make_service()),
    );
    tokio::spawn(axum_server::bind(plain_port.socket_addr()).serve(app.into_make_service()));

    let tls_url = tls_port.https_url("/");
    let plain_url = plain_port.http_url("/");
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("h2", &proxy_port)])
        .proxies(vec![
            http_proxy_entry("tls", "h2", proxy("tls.localhost", tls_url.as_str(), false)),
            http_proxy_entry(
                "plain",
                "h2",
                proxy("plain.localhost", plain_url.as_str(), false),
            ),
            http_proxy_entry(
                "h2c",
                "h2",
                proxy("h2c.localhost", plain_url.as_str(), true),
            ),
        ])
        .certs(
            [(root.id, root.clone()), (cert.id, cert.clone())]
                .into_iter()
                .collect(),
        )
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        let upstream_version = |host: &'static str| {
            client
                .get(proxy_port.http_url("/version"))
                .header(HOST, host)
                .send()
        };

        // The upstream TLS server selects h2 with ALPN.
        let resp = upstream_version("tls.localhost").await?;
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await?, "HTTP/2.0");

        // A plain upstream server receives HTTP/1.1 by default.
        let resp = upstream_version("plain.localhost").await?;
        assert_eq!(resp.text().await?, "HTTP/1.1");

        // The h2c option sends HTTP/2 with prior knowledge to the same plain server.
        let resp = upstream_version("h2c.localhost").await?;
        assert_eq!(resp.text().await?, "HTTP/2.0");

        // Requests share the upstream HTTP/2 connection.
        let resp = upstream_version("tls.localhost").await?;
        assert_eq!(resp.text().await?, "HTTP/2.0");
        Ok(())
    })
    .await
}
