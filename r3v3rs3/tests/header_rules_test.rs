use mockito::Matcher;
use r3v3rs3_api::{
    header_rules::{HeaderRule, HeaderRules},
    proxy::{HttpProxy, Route},
};
use reqwest::header::VARY;

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

fn rules(lines: &[&str]) -> Vec<HeaderRule> {
    lines
        .iter()
        .map(|line| HeaderRule::parse_line(line).unwrap())
        .collect()
}

#[tokio::test]
async fn header_rules_change_requests_and_responses() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let client_ip = port.socket_addr().ip().to_string();
    let mut upstream = mockito::Server::new_async().await;
    let mock_app = upstream
        .mock("GET", "/app")
        .match_header("x-client", client_ip.as_str())
        .match_header("x-info", "http / {literal}")
        .match_header("x-request-id", Matcher::Regex("^[0-9a-f]{32}$".into()))
        .match_header("x-secret", Matcher::Missing)
        .with_header("server", "mock")
        .with_header("vary", "Origin")
        .with_body("app")
        .expect(1)
        .create_async()
        .await;
    let mock_api = upstream
        .mock("GET", "/api")
        .match_header("x-tag", "api")
        .match_header("x-secret", "kept")
        .with_body("api")
        .expect(1)
        .create_async()
        .await;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![
            Route {
                headers: Some(HeaderRules {
                    request: rules(&["set X-Tag: api"]),
                    response: vec![],
                }),
                ..http_route("/api", &format!("{}/api", upstream.url()), None)
            },
            http_route("/", &upstream.url(), None),
        ],
        upgrade_insecure: false,
        headers: HeaderRules {
            request: rules(&[
                "set X-Client: {client_ip}",
                "set X-Info: {scheme} {route} {{literal}}",
                "set X-Request-Id: {request_id}",
                "remove X-Secret",
            ]),
            response: rules(&[
                "set X-Frame-Options: DENY",
                "remove Server",
                "append Vary: Accept",
            ]),
        },
        ..Default::default()
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("headers", &port)])
        .proxies(vec![http_proxy_entry("proxy1", "headers", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();

        let resp = client
            .get(port.http_url("/app"))
            .header("x-secret", "kept")
            .send()
            .await?;
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers()["x-frame-options"], "DENY");
        assert!(resp.headers().get("server").is_none());
        assert_eq!(
            resp.headers().get_all(VARY).iter().collect::<Vec<_>>(),
            ["Origin", "Accept"]
        );
        assert_eq!(resp.text().await?, "app");

        // The route rules replace the proxy rules.
        let resp = client
            .get(port.http_url("/api"))
            .header("x-secret", "kept")
            .send()
            .await?;
        assert_eq!(resp.status(), 200);
        assert!(resp.headers().get("x-frame-options").is_none());
        assert_eq!(resp.text().await?, "api");

        Ok(())
    })
    .await?;

    mock_app.assert_async().await;
    mock_api.assert_async().await;
    Ok(())
}

#[tokio::test]
async fn hop_by_hop_and_connection_headers_are_stripped_before_forwarding() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut upstream = mockito::Server::new_async().await;
    let mock_upstream = upstream
        .mock("GET", "/test")
        .match_header("x-kept", "preserved")
        .match_header("x-drop-me", Matcher::Missing)
        .match_header("keep-alive", Matcher::Missing)
        .match_header("proxy-connection", Matcher::Missing)
        .match_header("upgrade", Matcher::Missing)
        .with_body("ok")
        .expect(1)
        .create_async()
        .await;

    let storage = TestStorage::builder()
        .ports(vec![http_port_entry("phop", &port)])
        .proxies(vec![http_proxy_entry(
            "pxhop",
            "phop",
            HttpProxy {
                routes: vec![http_route("/test", &format!("{}/test", upstream.url()), None)],
                ..Default::default()
            },
        )])
        .build();

    with_server(storage, |_| async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::TcpStream::connect(port.socket_addr()).await?;
        let raw_req = format!(
            "GET /test HTTP/1.1\r\nHost: {}\r\nConnection: close, X-Drop-Me\r\nX-Drop-Me: secret-value\r\nKeep-Alive: 300\r\nProxy-Connection: keep-alive\r\nUpgrade: h2c\r\nX-Kept: preserved\r\n\r\n",
            port.socket_addr()
        );
        stream.write_all(raw_req.as_bytes()).await?;

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await?;
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 200 OK"), "unexpected response: {resp}");
        mock_upstream.assert_async().await;
        Ok(())
    })
    .await
}
