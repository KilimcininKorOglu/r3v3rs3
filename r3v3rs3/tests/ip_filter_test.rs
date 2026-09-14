use ipnet::IpNet;
use mockito::Matcher;
use r3v3rs3_api::{
    client_ip::ClientIpConfig,
    i18n::Locale,
    policy::IpFilter,
    proxy::{HttpProxy, Route},
};
use reqwest::{header::COOKIE, StatusCode};

mod common;
use common::{
    alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server, TestStorage,
};

fn nets(values: &[&str]) -> Vec<IpNet> {
    values.iter().map(|value| value.parse().unwrap()).collect()
}

fn loopback() -> Vec<IpNet> {
    nets(&["127.0.0.0/8", "::1/128"])
}

fn proxy(routes: Vec<Route>, ip_filter: IpFilter, client_ip: ClientIpConfig) -> HttpProxy {
    HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes,
        upgrade_insecure: false,
        client_ip,
        ip_filter,
        rate_limit: Default::default(),
        auth: Default::default(),
        headers: Default::default(),
        compression: Default::default(),
        cache: Default::default(),
        h2c: false,
        timeouts: Default::default(),
        load_balancing: Default::default(),
        health_check: Default::default(),
        circuit_breaker: Default::default(),
        client_cert: None,
    }
}

#[tokio::test]
async fn ip_filter_allows_and_denies_clients() -> anyhow::Result<()> {
    let deny_port = alloc_tcp_port().await?;
    let allow_port = alloc_tcp_port().await?;
    let forwarded_port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let upstream = server.url();

    let mock_root = server
        .mock("GET", "/")
        .with_body("root")
        .expect(1)
        .create_async()
        .await;
    let mock_allowed = server
        .mock("GET", "/allowed")
        .with_body("allowed")
        .expect(2)
        .create_async()
        .await;
    let mock_denied = server
        .mock("GET", Matcher::Regex("^/(denied|spoofed)$".into()))
        .expect(0)
        .create_async()
        .await;

    let deny_loopback = proxy(
        vec![
            http_route("/open", &upstream, Some(IpFilter::default())),
            http_route("/", &upstream, None),
        ],
        IpFilter {
            allow: vec![],
            deny: loopback(),
        },
        ClientIpConfig::default(),
    );
    let allow_loopback = proxy(
        vec![
            http_route(
                "/admin",
                &upstream,
                Some(IpFilter {
                    allow: nets(&["10.0.0.0/8"]),
                    deny: vec![],
                }),
            ),
            http_route("/", &upstream, None),
        ],
        IpFilter {
            allow: loopback(),
            deny: vec![],
        },
        ClientIpConfig::default(),
    );
    let deny_forwarded = proxy(
        vec![http_route("/", &upstream, None)],
        IpFilter {
            allow: vec![],
            deny: nets(&["203.0.113.0/24"]),
        },
        ClientIpConfig {
            trust_cdn: true,
            trusted_proxies: loopback(),
        },
    );

    let config = TestStorage::builder()
        .ports(vec![
            http_port_entry("deny", &deny_port),
            http_port_entry("allow", &allow_port),
            http_port_entry("fwd", &forwarded_port),
        ])
        .proxies(vec![
            http_proxy_entry("proxy1", "deny", deny_loopback),
            http_proxy_entry("proxy2", "allow", allow_loopback),
            http_proxy_entry("proxy3", "fwd", deny_forwarded),
        ])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();

        let resp = client.get(deny_port.http_url("/open")).send().await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await?, "root");

        let resp = client.get(deny_port.http_url("/denied")).send().await?;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(resp.headers()["content-type"], "text/html; charset=utf-8");
        let body = resp.text().await?;
        assert!(body.contains("<div class=\"error-text\">Forbidden</div>"));
        assert!(body.contains(r#"<html lang="en">"#), "{body}");

        // The error page uses the language and the theme that the WebUI stores in cookies.
        let resp = client
            .get(deny_port.http_url("/denied"))
            .header(COOKIE, "r3v3rs3_lang=tr; r3v3rs3_theme=dark")
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = resp.text().await?;
        assert!(
            body.contains(r#"<html lang="tr" data-theme="dark">"#),
            "{body}"
        );
        assert!(body.contains(&format!(
            "<div class=\"error-text\">{}</div>",
            Locale::Tr.t("error_page.403")
        )));

        let resp = client.get(allow_port.http_url("/allowed")).send().await?;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = client.get(allow_port.http_url("/admin")).send().await?;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let resp = client
            .get(forwarded_port.http_url("/spoofed"))
            .header("x-forwarded-for", "203.0.113.9")
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let resp = client
            .get(forwarded_port.http_url("/allowed"))
            .header("x-forwarded-for", "198.51.100.1")
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);

        Ok(())
    })
    .await?;

    mock_root.assert_async().await;
    mock_allowed.assert_async().await;
    mock_denied.assert_async().await;
    Ok(())
}
