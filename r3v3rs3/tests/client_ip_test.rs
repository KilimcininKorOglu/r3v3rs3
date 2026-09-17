use mockito::Matcher;
use r3v3rs3_api::{client_ip::ClientIpConfig, proxy::HttpProxy};

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

fn proxy(upstream: &str, client_ip: ClientIpConfig) -> HttpProxy {
    HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", upstream, None)],
        upgrade_insecure: false,
        client_ip,
        ..Default::default()
    }
}

#[tokio::test]
async fn client_ip_from_trusted_proxy() -> anyhow::Result<()> {
    let trusted_port = alloc_tcp_port().await?;
    let direct_port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;

    let mock_trusted = server
        .mock("GET", "/trusted")
        .match_header("x-real-ip", "203.0.113.9")
        .match_header(
            "x-forwarded-for",
            Matcher::Regex(r"^203\.0\.113\.9, ".into()),
        )
        .with_body("trusted")
        .create_async()
        .await;

    let mock_direct = server
        .mock("GET", "/direct")
        .match_header("x-real-ip", Matcher::Missing)
        .match_header("cf-connecting-ip", Matcher::Missing)
        .match_header("x-forwarded-for", Matcher::Regex(r"^[^,]+$".into()))
        .with_body("direct")
        .create_async()
        .await;

    let trusted = ClientIpConfig {
        trust_cdn: true,
        trusted_proxies: vec!["127.0.0.0/8".parse()?, "::1/128".parse()?],
    };

    let config = TestStorage::builder()
        .ports(vec![
            http_port_entry("trusted", &trusted_port),
            http_port_entry("direct", &direct_port),
        ])
        .proxies(vec![
            http_proxy_entry("proxy1", "trusted", proxy(&server.url(), trusted)),
            http_proxy_entry(
                "proxy2",
                "direct",
                proxy(&server.url(), ClientIpConfig::default()),
            ),
        ])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();

        let resp = client
            .get(trusted_port.http_url("/trusted"))
            .header("x-forwarded-for", "203.0.113.9")
            .send()
            .await?;
        assert_eq!(resp.text().await?, "trusted");

        let resp = client
            .get(direct_port.http_url("/direct"))
            .header("x-forwarded-for", "203.0.113.9")
            .header("x-real-ip", "203.0.113.9")
            .header("cf-connecting-ip", "203.0.113.9")
            .send()
            .await?;
        assert_eq!(resp.text().await?, "direct");

        Ok(())
    })
    .await?;

    mock_trusted.assert_async().await;
    mock_direct.assert_async().await;
    Ok(())
}
