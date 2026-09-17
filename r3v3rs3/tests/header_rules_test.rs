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
