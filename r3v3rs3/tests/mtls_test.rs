use axum::{http::HeaderMap, routing::get, Router};
use axum_server::tls_rustls::RustlsConfig;
use r3v3rs3::{admin::start_admin, certs::Cert, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::{
    header_rules::{HeaderRule, HeaderRules},
    id::ShortId,
    multiaddr::Multiaddr,
    port::{Port, PortEntry, PortOptions, UpstreamServer},
    proxy::{HttpProxy, ProxyEntry, ProxyKind, TcpProxy},
    tls::{ClientAuthMode, TlsTermination},
};
use reqwest::{header::COOKIE, Identity, StatusCode};
use std::{collections::HashMap, future::IntoFuture, sync::Arc};
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{
    admin_session_cookie, alloc_tcp_port, http_proxy_entry, http_route, port_entry,
    wait_for_listener, with_server, TestPort, TestStorage,
};

/// A root certificate with a server certificate and a client certificate that it signs.
struct Pki {
    root: Arc<Cert>,
    server: Arc<Cert>,
    client: Arc<Cert>,
}

impl Pki {
    fn new() -> Self {
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

    fn certs(&self) -> HashMap<ShortId, Arc<Cert>> {
        [&self.root, &self.server, &self.client]
            .into_iter()
            .map(|cert| (cert.id, cert.clone()))
            .collect()
    }

    /// Returns a client that trusts the root certificate and sends the client certificate.
    fn client(&self, identity: Option<&Cert>) -> anyhow::Result<reqwest::Client> {
        let builder = reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(&self.root.pem_chain)?);
        let builder = match identity {
            Some(cert) => builder.identity(identity_of(cert)?),
            None => builder,
        };
        Ok(builder.build()?)
    }
}

fn identity_of(cert: &Cert) -> anyhow::Result<Identity> {
    let key = cert
        .pem_key
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("certificate has no private key"))?;
    Ok(Identity::from_pem(
        &[key.as_slice(), &cert.pem_chain].concat(),
    )?)
}

/// A client certificate that another root certificate signs.
fn foreign_client_cert() -> Cert {
    let root = Cert::new_ca().unwrap();
    Cert::new_client(&["client.example.com".parse().unwrap()], &root).unwrap()
}

fn tls_port(id: &str, listen: Multiaddr, mode: ClientAuthMode, roots: Vec<ShortId>) -> PortEntry {
    let mut entry = port_entry(id, listen);
    entry.port.opts = PortOptions {
        tls_termination: Some(TlsTermination {
            server_names: vec!["localhost".into()],
            client_auth: mode,
            client_ca_certs: roots,
        }),
    };
    entry
}

/// Starts a plain HTTP upstream server that returns the client certificate headers.
async fn start_echo_upstream() -> anyhow::Result<TestPort> {
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

fn https_storage(
    pki: &Pki,
    mode: ClientAuthMode,
    proxy_port: &TestPort,
    upstream: &TestPort,
) -> TestStorage {
    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", upstream.http_url("/").as_str(), None)],
        headers: HeaderRules {
            request: vec![
                HeaderRule::parse_line("set X-Client-Cert: {client_cert_subject}").unwrap(),
                HeaderRule::parse_line("set X-Client-Cert-Fingerprint: {client_cert_fingerprint}")
                    .unwrap(),
            ],
            response: vec![],
        },
        ..Default::default()
    };
    TestStorage::builder()
        .ports(vec![tls_port(
            "mtls",
            proxy_port.multiaddr_https(),
            mode,
            vec![pki.root.id],
        )])
        .proxies(vec![http_proxy_entry("proxy", "mtls", proxy)])
        .certs(pki.certs())
        .build()
}

/// Starts an HTTPS port in the mode and checks it with a client without a certificate, a client
/// with a certificate of another root and a client with a valid certificate. The client sends a
/// forged `X-Client-Cert` header, which the header rule must replace.
async fn check_https_client_auth(mode: ClientAuthMode) -> anyhow::Result<()> {
    let pki = Pki::new();
    let upstream = start_echo_upstream().await?;
    let proxy_port = alloc_tcp_port().await?;
    let storage = https_storage(&pki, mode, &proxy_port, &upstream);
    let url = proxy_port.https_url("/");
    let expected = format!("CN=client.example.com|{}", pki.client.fingerprint);

    with_server(storage, |_| async move {
        let get = |identity: Option<&Cert>| {
            let client = pki.client(identity);
            let url = url.clone();
            async move {
                let resp = client?
                    .get(url)
                    .header("x-client-cert", "CN=forged")
                    .send()
                    .await?;
                anyhow::Ok(resp.text().await?)
            }
        };

        let anonymous = get(None).await;
        match mode {
            ClientAuthMode::Required => assert!(anonymous.is_err(), "{anonymous:?}"),
            _ => assert_eq!(anonymous?, "|"),
        }

        // Both modes reject a certificate that the selected root certificates do not verify.
        let foreign = foreign_client_cert();
        let result = get(Some(&foreign)).await;
        assert!(result.is_err(), "a certificate of another root: {result:?}");

        assert_eq!(get(Some(&pki.client)).await?, expected);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn https_required_client_auth() -> anyhow::Result<()> {
    check_https_client_auth(ClientAuthMode::Required).await
}

#[tokio::test]
async fn https_optional_client_auth() -> anyhow::Result<()> {
    check_https_client_auth(ClientAuthMode::Optional).await
}

#[tokio::test]
async fn tls_required_client_auth() -> anyhow::Result<()> {
    let pki = Pki::new();
    let upstream = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;

    let config = RustlsConfig::from_pem(
        pki.server.pem_chain.clone(),
        pki.server.pem_key.clone().unwrap(),
    )
    .await?;
    let app = Router::new().route("/hello", get(|| async { "Hello" }));
    tokio::spawn(
        axum_server::bind_rustls(upstream.socket_addr(), config).serve(app.into_make_service()),
    );

    let proxy = ProxyEntry {
        id: "proxy".parse().unwrap(),
        proxy: r3v3rs3_api::proxy::Proxy {
            ports: vec!["mtls".parse().unwrap()],
            kind: ProxyKind::Tcp(TcpProxy {
                upstream_servers: vec![UpstreamServer {
                    addr: format!("/dns/localhost/tcp/{}/tls", upstream.socket_addr().port())
                        .parse()
                        .unwrap(),
                }],
            }),
            ..Default::default()
        },
    };
    let storage = TestStorage::builder()
        .ports(vec![tls_port(
            "mtls",
            proxy_port.multiaddr_tls(),
            ClientAuthMode::Required,
            vec![pki.root.id],
        )])
        .proxies(vec![proxy])
        .certs(pki.certs())
        .build();
    let url = proxy_port.https_url("/hello");

    with_server(storage, |_| async move {
        let result = pki.client(None)?.get(url.clone()).send().await;
        assert!(
            result.is_err(),
            "a client without a certificate: {result:?}"
        );

        let resp = pki.client(Some(&pki.client))?.get(url).send().await?;
        assert_eq!(resp.text().await?, "Hello");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn admin_api_checks_client_ca_certs() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-mtls-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;

    let pki = Pki::new();
    let addr = alloc_tcp_port().await?.socket_addr();
    let listen = alloc_tcp_port().await?.multiaddr_https();
    let storage = TestStorage::builder()
        .accounts(HashMap::from([("admin".to_string(), "secret".to_string())]))
        .certs(pki.certs())
        .build();

    let app_info = new_appinfo(&dir, &dir);
    let root_id = pki.root.id;
    let server_id = pki.server.id;
    with_server(storage, |channels| async move {
        tokio::spawn(start_admin(
            app_info,
            addr,
            channels.command,
            channels.callback,
            channels.event.clone(),
        ));
        wait_for_listener(addr).await?;
        let cookie = admin_session_cookie(addr).await?;
        let client = reqwest::Client::new();
        let add_port = |roots: Vec<ShortId>| {
            let port: Port =
                tls_port("unused", listen.clone(), ClientAuthMode::Required, roots).port;
            client
                .post(format!("http://{addr}/api/ports"))
                .header(COOKIE, &cookie)
                .json(&port)
                .send()
        };

        for (roots, error) in [
            (vec![], "client_ca_certs_missing"),
            (vec![server_id], "invalid_client_ca_cert"),
        ] {
            let resp = add_port(roots).await?;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            let body = resp.text().await?;
            assert!(body.contains(error), "{body}");
        }

        add_port(vec![root_id]).await?.error_for_status()?;

        let resp = client
            .delete(format!("http://{addr}/api/certs/{root_id}"))
            .header(COOKIE, &cookie)
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.text().await?;
        assert!(body.contains("certificate_in_use"), "{body}");

        client
            .delete(format!("http://{addr}/api/certs/{server_id}"))
            .header(COOKIE, &cookie)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
