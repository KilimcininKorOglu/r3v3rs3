use r3v3rs3_api::port::Port;
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::priority::{
    GLOBAL_STREAM_CONTROLLER, MimePriorityRule, StreamPriority, StreamPriorityConfig,
    parse_rfc9218_priority, resolve_stream_priority,
};
use r3v3rs3_api::proxy::{HttpProxy, Proxy, ProxyEntry, ProxyKind, Route, Server};

mod common;
use common::{TestStorage, alloc_tcp_port, wait_for_listener, with_server};

#[tokio::test]
async fn test_rfc9218_spec_compliance() {
    // RFC 9218 dictionary syntax: u=0..7, i
    assert_eq!(
        parse_rfc9218_priority("u=0"),
        Some(StreamPriority::new(0, false))
    );
    assert_eq!(
        parse_rfc9218_priority("u=3, i"),
        Some(StreamPriority::new(3, true))
    );
    assert_eq!(
        parse_rfc9218_priority("i=?1, u=6"),
        Some(StreamPriority::new(6, true))
    );
    assert_eq!(
        parse_rfc9218_priority("u=7, i=?0"),
        Some(StreamPriority::new(7, false))
    );

    // Serialization
    assert_eq!(StreamPriority::new(0, false).to_header_value(), "u=0");
    assert_eq!(StreamPriority::new(5, true).to_header_value(), "u=5, i");

    // Weights
    assert_eq!(StreamPriority::new(0, false).weight(), 256);
    assert_eq!(StreamPriority::new(1, false).weight(), 128);
    assert_eq!(StreamPriority::new(7, true).weight(), 2);
}

#[tokio::test]
async fn test_mime_heuristics_and_custom_rules() {
    let config = StreamPriorityConfig {
        enabled: true,
        inject_header: true,
        adaptive_concurrency: true,
        custom_rules: vec![
            MimePriorityRule {
                pattern: "/api/urgent/*".into(),
                urgency: 0,
                incremental: false,
            },
            MimePriorityRule {
                pattern: "image/svg+xml".into(),
                urgency: 2,
                incremental: false,
            },
        ],
    };

    // Custom rule matches
    let urgent = resolve_stream_priority(None, "/api/urgent/checkout", None, &config);
    assert_eq!(urgent.urgency, 0);
    assert!(!urgent.incremental);

    let svg = resolve_stream_priority(None, "/logo.svg", Some("image/svg+xml"), &config);
    assert_eq!(svg.urgency, 2);

    // Built-in heuristics
    let html = resolve_stream_priority(None, "/index.html", Some("text/html"), &config);
    assert_eq!(html.urgency, 0);

    let css = resolve_stream_priority(None, "/assets/app.css", Some("text/css"), &config);
    assert_eq!(css.urgency, 1);

    let js = resolve_stream_priority(None, "/app.mjs", None, &config);
    assert_eq!(js.urgency, 1);

    let font = resolve_stream_priority(None, "/fonts/inter.woff2", None, &config);
    assert_eq!(font.urgency, 2);

    let img = resolve_stream_priority(None, "/photo.webp", Some("image/webp"), &config);
    assert_eq!(img.urgency, 5);
    assert!(img.incremental);

    let video = resolve_stream_priority(None, "/stream.mp4", Some("video/mp4"), &config);
    assert_eq!(video.urgency, 6);
    assert!(video.incremental);

    // Client header takes precedence when present
    let client_override =
        resolve_stream_priority(Some("u=0, i"), "/photo.webp", Some("image/webp"), &config);
    assert_eq!(client_override.urgency, 0);
    assert!(client_override.incremental);
}

#[tokio::test]
async fn test_controller_backpressure_and_counters() {
    let initial_stats = GLOBAL_STREAM_CONTROLLER.stats();

    let crit = StreamPriority::new(0, false);
    let bg = StreamPriority::new(6, true);

    GLOBAL_STREAM_CONTROLLER.record_request(&crit);
    GLOBAL_STREAM_CONTROLLER.record_request(&bg);

    let stats = GLOBAL_STREAM_CONTROLLER.stats();
    assert!(stats.total_requests >= initial_stats.total_requests + 2);
    assert!(stats.critical_requests > initial_stats.critical_requests);
    assert!(stats.background_requests > initial_stats.background_requests);
}

#[tokio::test]
async fn test_proxy_injects_priority_header() -> anyhow::Result<()> {
    let mut server = mockito::Server::new_async().await;
    let mock_html = server
        .mock("GET", "/page.html")
        .match_header("priority", "u=0")
        .with_body("html-ok")
        .create_async()
        .await;

    let mock_css = server
        .mock("GET", "/style.css")
        .match_header("priority", "u=1")
        .with_body("css-ok")
        .create_async()
        .await;

    let mock_img = server
        .mock("GET", "/picture.webp")
        .match_header("priority", "u=5, i")
        .with_body("img-ok")
        .create_async()
        .await;

    let mock_client = server
        .mock("GET", "/custom.webp")
        .match_header("priority", "u=2")
        .with_body("custom-ok")
        .create_async()
        .await;

    let proxy_port = alloc_tcp_port().await?;
    let config = TestStorage::builder()
        .ports(vec![PortEntry {
            id: "port1".parse().unwrap(),
            port: Port {
                active: true,
                name: "http".into(),
                listen: proxy_port.multiaddr_http(),
                opts: Default::default(),
            },
        }])
        .proxies(vec![ProxyEntry {
            id: "proxy1".parse().unwrap(),
            source: None,
            proxy: Proxy {
                active: true,
                name: "priority-proxy".into(),
                ports: vec!["port1".parse().unwrap()],
                kind: ProxyKind::Http(Box::new(HttpProxy {
                    routes: vec![Route {
                        path: "/".into(),
                        servers: vec![Server::new(server.url().parse()?)],
                        ..Default::default()
                    }],
                    stream_priority: StreamPriorityConfig {
                        enabled: true,
                        inject_header: true,
                        adaptive_concurrency: true,
                        custom_rules: vec![],
                    },
                    ..Default::default()
                })),
            },
        }])
        .build();

    with_server(config, |_| async move {
        wait_for_listener(proxy_port.socket_addr()).await?;
        let client = reqwest::Client::new();
        let base_url = proxy_port.http_url("");

        // 1. Request to HTML document -> should receive Priority: u=0
        let resp_html = client
            .get(format!("{base_url}/page.html"))
            .send()
            .await?
            .text()
            .await?;
        assert_eq!(resp_html, "html-ok");
        mock_html.assert_async().await;

        // 2. Request to CSS -> should receive Priority: u=1
        let resp_css = client
            .get(format!("{base_url}/style.css"))
            .send()
            .await?
            .text()
            .await?;
        assert_eq!(resp_css, "css-ok");
        mock_css.assert_async().await;

        // 3. Request to image -> should receive Priority: u=5, i
        let resp_img = client
            .get(format!("{base_url}/picture.webp"))
            .send()
            .await?
            .text()
            .await?;
        assert_eq!(resp_img, "img-ok");
        mock_img.assert_async().await;

        // 4. Request with explicit client Priority header -> preserved!
        let resp_custom = client
            .get(format!("{base_url}/custom.webp"))
            .header("priority", "u=2")
            .send()
            .await?
            .text()
            .await?;
        assert_eq!(resp_custom, "custom-ok");
        mock_client.assert_async().await;

        Ok(())
    })
    .await
}
