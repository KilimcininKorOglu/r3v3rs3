use r3v3rs3_api::{client_ip::ClientIpConfig, policy::IpFilter, proxy::HttpProxy};

mod common;
use common::{
    TestPort, TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route,
    with_server,
};

/// The client address that every request names through the trusted `X-Forwarded-For` header.
const CLIENT: &str = "203.0.113.160";

/// The default number of violations that quarantines an address.
const VIOLATIONS: usize = 5;

/// A proxy that trusts loopback as a proxy, serves `/` from the upstream server and refuses
/// `/private` for the documentation network with its own IP filter.
fn proxy(upstream: &str) -> anyhow::Result<HttpProxy> {
    let deny = IpFilter {
        deny: vec!["203.0.113.0/24".parse()?],
        ..Default::default()
    };
    Ok(HttpProxy {
        vhosts: vec!["localhost".parse()?],
        routes: vec![
            http_route("/private", upstream, Some(deny)),
            http_route("/", upstream, None),
        ],
        client_ip: ClientIpConfig {
            trust_cdn: false,
            trusted_proxies: vec!["127.0.0.0/8".parse()?, "::1/128".parse()?],
        },
        ..Default::default()
    })
}

/// Sends `count` requests for `path` from [`CLIENT`] and returns the last status.
async fn send_many(port: &TestPort, path: &str, count: usize) -> anyhow::Result<u16> {
    let client = reqwest::Client::new();
    let mut status = 0;
    for _ in 0..count {
        let resp = client
            .get(port.http_url(path))
            .header("x-forwarded-for", CLIENT)
            .send()
            .await?;
        status = resp.status().as_u16();
    }
    Ok(status)
}

/// Runs a server with [`proxy`] in front of an upstream that answers `/login` with 403 and
/// `/` with 200, then runs `check`.
async fn with_proxy<F, O>(check: F) -> anyhow::Result<()>
where
    F: FnOnce(TestPort) -> O,
    O: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let _login = server
        .mock("GET", "/login")
        .with_status(403)
        .expect_at_least(1)
        .create_async()
        .await;
    let _home = server.mock("GET", "/").create_async().await;
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry(
            "qrn-test",
            "http",
            proxy(&server.url())?,
        )])
        .build();
    with_server(config, |_| check(port)).await
}

#[tokio::test]
async fn forbidden_responses_of_the_upstream_never_quarantine() -> anyhow::Result<()> {
    with_proxy(|port| async move {
        assert_eq!(send_many(&port, "/login", VIOLATIONS * 2).await?, 403);
        assert_eq!(send_many(&port, "/", 1).await?, 200);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn forbidden_responses_of_r3v3rs3_quarantine() -> anyhow::Result<()> {
    with_proxy(|port| async move {
        assert_eq!(send_many(&port, "/private", VIOLATIONS).await?, 403);
        assert!(
            send_many(&port, "/", 1).await.is_err(),
            "the quarantine drops the connection"
        );
        Ok(())
    })
    .await
}
