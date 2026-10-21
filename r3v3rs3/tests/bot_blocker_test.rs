use r3v3rs3_api::proxy::HttpProxy;
use reqwest::StatusCode;

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

#[tokio::test]
async fn test_bot_blocker_blocks_ai_scrapers() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let upstream = server.url();

    let _mock = server
        .mock("GET", "/")
        .with_status(200)
        .with_body("content for humans")
        .create_async()
        .await;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream, None)],
        bot_blocker: true,
        ..Default::default()
    };

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("bot-test", "http", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        let url = port.http_url("/");

        // Regular browser is allowed
        let res = client
            .get(url.clone())
            .header("Host", "localhost")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
            )
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);

        // GPTBot is blocked
        let res = client
            .get(url.clone())
            .header("Host", "localhost")
            .header(
                "User-Agent",
                "Mozilla/5.0 (compatible; GPTBot/1.2; +https://openai.com/gptbot)",
            )
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // ClaudeBot is blocked
        let res = client
            .get(url.clone())
            .header("Host", "localhost")
            .header("User-Agent", "ClaudeBot/1.0; +https://anthropic.com")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // Bytespider is blocked
        let res = client
            .get(url.clone())
            .header("Host", "localhost")
            .header(
                "User-Agent",
                "Mozilla/5.0 (compatible; Bytespider; spider-feedback@bytedance.com)",
            )
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // CCBot is blocked
        let res = client
            .get(url.clone())
            .header("Host", "localhost")
            .header("User-Agent", "CCBot/2.0 (https://commoncrawl.org/faq/)")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // PerplexityBot is blocked
        let res = client
            .get(url.clone())
            .header("Host", "localhost")
            .header(
                "User-Agent",
                "PerplexityBot/1.0 (+https://perplexity.ai/perplexitybot)",
            )
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // Scrapy scraper is blocked
        let res = client
            .get(url)
            .header("Host", "localhost")
            .header("User-Agent", "Scrapy/2.11.0 (+https://scrapy.org)")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_bot_blocker_disabled_allows_bots() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let upstream = server.url();

    let _mock = server
        .mock("GET", "/")
        .with_status(200)
        .with_body("content for all")
        .create_async()
        .await;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream, None)],
        bot_blocker: false,
        ..Default::default()
    };

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("allowbot", "http", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        let url = port.http_url("/");

        let res = client
            .get(url)
            .header("Host", "localhost")
            .header(
                "User-Agent",
                "Mozilla/5.0 (compatible; GPTBot/1.2; +https://openai.com/gptbot)",
            )
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_bot_blocker_route_override() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let upstream = server.url();

    let _mock = server
        .mock("GET", "/")
        .with_status(200)
        .with_body("upstream response")
        .create_async()
        .await;

    let mut route_public = http_route("/public", &upstream, None);
    route_public.bot_blocker = Some(false);

    let mut route_private = http_route("/private", &upstream, None);
    route_private.bot_blocker = Some(true);

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![route_public, route_private],
        bot_blocker: true,
        ..Default::default()
    };

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("override", "http", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();

        // /public route overrides to false, so GPTBot is allowed
        let res = client
            .get(port.http_url("/public"))
            .header("Host", "localhost")
            .header("User-Agent", "GPTBot/1.2")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);

        // /private route overrides to true, so GPTBot is blocked
        let res = client
            .get(port.http_url("/private"))
            .header("Host", "localhost")
            .header("User-Agent", "GPTBot/1.2")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        Ok(())
    })
    .await
}
