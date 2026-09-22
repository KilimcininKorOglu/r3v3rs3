use r3v3rs3::certs::Cert;
use r3v3rs3_api::{
    header_rules::{HeaderRule, HeaderRules},
    proxy::HttpProxy,
    tls::ClientAuthMode,
};
use reqwest::StatusCode;

mod common;
use common::{
    TestPort, TestStorage, alloc_udp_port, http_proxy_entry, http_route,
    pki::{Pki, foreign_client_cert, start_echo_upstream, tls_port},
    quic::{h3_get, quic_endpoint, wait_for_quic},
    with_server,
};

/// A QUIC port with an HTTP proxy that copies the client certificate into request headers.
fn h3_storage(
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
            "h3",
            proxy_port.multiaddr_quic_https(),
            mode,
            match mode {
                ClientAuthMode::Off => vec![],
                _ => vec![pki.root.id],
            },
        )])
        .proxies(vec![http_proxy_entry("proxy", "h3", proxy)])
        .certs(pki.certs())
        .build()
}

/// Starts a QUIC port in the mode and checks it with a client without a certificate, a client with
/// a certificate of another root and a client with a valid certificate. The client sends a forged
/// `X-Client-Cert` header, which the header rule must replace.
async fn check_h3_client_auth(mode: ClientAuthMode) -> anyhow::Result<()> {
    let pki = Pki::new();
    let upstream = start_echo_upstream().await?;
    let proxy_port = alloc_udp_port().await?;
    let addr = proxy_port.socket_addr();
    let storage = h3_storage(&pki, mode, &proxy_port, &upstream);
    let url = proxy_port.https_url("/").to_string();
    let expected = format!("CN=client.example.com|{}", pki.client.fingerprint);

    with_server(storage, |_| async move {
        let get = async |identity: Option<&Cert>| {
            let endpoint = quic_endpoint(&pki.root, identity)?;
            let (status, body) = h3_get(
                &endpoint,
                addr,
                "localhost",
                &url,
                &[("x-client-cert", "CN=forged")],
            )
            .await?;
            anyhow::Ok((status, body))
        };

        // The valid client also proves the port listens, so it runs first.
        let valid = quic_endpoint(&pki.root, Some(&pki.client))?;
        wait_for_quic(&valid, addr, "localhost").await?;
        let (status, body) = get(Some(&pki.client)).await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, expected);

        let anonymous = get(None).await;
        match mode {
            ClientAuthMode::Required => assert!(anonymous.is_err(), "{anonymous:?}"),
            _ => assert_eq!(anonymous?.1, "|"),
        }

        // Both modes reject a certificate that the selected root certificates do not verify.
        let foreign = foreign_client_cert();
        let result = get(Some(&foreign)).await;
        assert!(result.is_err(), "a certificate of another root: {result:?}");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn http3_serves_a_request() -> anyhow::Result<()> {
    let pki = Pki::new();
    let upstream = start_echo_upstream().await?;
    let proxy_port = alloc_udp_port().await?;
    let addr = proxy_port.socket_addr();
    let storage = h3_storage(&pki, ClientAuthMode::Off, &proxy_port, &upstream);
    let url = proxy_port.https_url("/").to_string();

    with_server(storage, |_| async move {
        let endpoint = quic_endpoint(&pki.root, None)?;
        wait_for_quic(&endpoint, addr, "localhost").await?;
        let (status, body) = h3_get(&endpoint, addr, "localhost", &url, &[]).await?;
        assert_eq!(status, StatusCode::OK);
        // No client certificate, so both header rules write an empty value.
        assert_eq!(body, "|");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn http3_required_client_auth() -> anyhow::Result<()> {
    check_h3_client_auth(ClientAuthMode::Required).await
}

#[tokio::test]
async fn http3_optional_client_auth() -> anyhow::Result<()> {
    check_h3_client_auth(ClientAuthMode::Optional).await
}
