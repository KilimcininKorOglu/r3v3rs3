use r3v3rs3_api::client_ip::ClientIpConfig;
use r3v3rs3_api::port::Port;
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::priority::DEFAULT_MAX_BACKGROUND_CONCURRENCY;
use r3v3rs3_api::priority::{
    AdaptiveStreamController, MimePriorityRule, StreamPriority, StreamPriorityConfig,
    parse_rfc9218_priority, resolve_stream_priority,
};
use r3v3rs3_api::proxy::{HttpProxy, Proxy, ProxyEntry, ProxyKind, Route, Server};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::common;
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

/// Uses its own controller: the global one also counts the proxy requests of the other tests
/// that run in this process.
#[tokio::test]
async fn test_controller_backpressure_and_counters() {
    let controller = std::sync::Arc::new(AdaptiveStreamController::new());
    let crit = StreamPriority::new(0, false);
    let normal = StreamPriority::new(3, false);
    let bg = StreamPriority::new(6, true);

    controller.record_request(&crit);
    controller.record_request(&normal);
    controller.record_request(&bg);
    let stats = controller.stats();
    assert_eq!(stats.total_requests, 3);
    assert_eq!(stats.critical_requests, 1);
    assert_eq!(stats.normal_requests, 1);
    assert_eq!(stats.background_requests, 1);

    // A critical stream is no background stream, so it does not count toward the limit.
    let client: std::net::IpAddr = [192, 0, 2, 1].into();
    let crit_permit = controller.try_acquire(crit, client, Some(1));
    assert!(crit_permit.is_some());
    let bg_permit = controller.try_acquire(bg, client, Some(1));
    assert!(bg_permit.is_some());
    assert_eq!(controller.stats().active_streams, 2);

    // The limit is reached when the background streams of the client equal it.
    assert!(controller.try_acquire(bg, client, Some(1)).is_none());
    assert_eq!(controller.stats().throttled_background, 1);
    assert_eq!(controller.stats().active_streams, 2);

    drop(bg_permit);
    drop(crit_permit);
    assert!(controller.try_acquire(bg, client, Some(1)).is_some());
    assert_eq!(controller.stats().active_streams, 0);
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

    // RFC 9218 section 4: an out-of-range urgency is ignored, so the heuristic of the path
    // applies instead of a clamped `u=7`.
    let mock_invalid = server
        .mock("GET", "/invalid.webp")
        .match_header("priority", "u=5, i")
        .with_body("invalid-ok")
        .create_async()
        .await;

    let proxy_port = alloc_tcp_port().await?;
    let config = priority_storage(&proxy_port, &server.url())?;

    with_server(config, |_| async move {
        wait_for_listener(proxy_port.socket_addr()).await?;
        let client = reqwest::Client::new();
        let base_url = proxy_port.http_url("");

        // 1. Request to HTML document -> should receive Priority: u=0
        let resp_html = fetch_text(&client, format!("{base_url}/page.html"), None).await?;
        assert_eq!(resp_html, "html-ok");
        mock_html.assert_async().await;

        // 2. Request to CSS -> should receive Priority: u=1
        let resp_css = fetch_text(&client, format!("{base_url}/style.css"), None).await?;
        assert_eq!(resp_css, "css-ok");
        mock_css.assert_async().await;

        // 3. Request to image -> should receive Priority: u=5, i
        let resp_img = fetch_text(&client, format!("{base_url}/picture.webp"), None).await?;
        assert_eq!(resp_img, "img-ok");
        mock_img.assert_async().await;

        // 4. Request with explicit client Priority header -> preserved!
        let resp_custom =
            fetch_text(&client, format!("{base_url}/custom.webp"), Some("u=2")).await?;
        assert_eq!(resp_custom, "custom-ok");
        mock_client.assert_async().await;

        let resp_invalid =
            fetch_text(&client, format!("{base_url}/invalid.webp"), Some("u=8")).await?;
        assert_eq!(resp_invalid, "invalid-ok");
        mock_invalid.assert_async().await;

        Ok(())
    })
    .await
}

/// Sends a GET, with the client `Priority` header when one is given, and returns the body.
async fn fetch_text(
    client: &reqwest::Client,
    url: String,
    priority: Option<&str>,
) -> anyhow::Result<String> {
    let mut request = client.get(url);
    if let Some(priority) = priority {
        request = request.header("priority", priority);
    }
    Ok(request.send().await?.text().await?)
}

/// Builds the storage with an HTTP proxy to the upstream that injects the `Priority` header.
fn priority_storage(proxy_port: &common::TestPort, upstream: &str) -> anyhow::Result<TestStorage> {
    Ok(TestStorage::builder()
        .ports(vec![PortEntry {
            id: "port1".parse().unwrap(),
            port: Port {
                active: true,
                name: "http".into(),
                listen: proxy_port.multiaddr_http(),
                opts: Default::default(),
            },
            status: None,
        }])
        .proxies(vec![ProxyEntry {
            id: "proxy1".parse().unwrap(),
            source: None,
            status: None,
            proxy: Proxy {
                active: true,
                name: "priority-proxy".into(),
                ports: vec!["port1".parse().unwrap()],
                kind: ProxyKind::Http(Box::new(HttpProxy {
                    routes: vec![Route {
                        path: "/".into(),
                        servers: vec![Server::new(upstream.parse()?)],
                        ..Default::default()
                    }],
                    // Without the background limit, so the limit test of this file, which also
                    // runs from 127.0.0.1, cannot refuse these requests.
                    stream_priority: StreamPriorityConfig {
                        enabled: true,
                        inject_header: true,
                        adaptive_concurrency: false,
                        custom_rules: vec![],
                    },
                    ..Default::default()
                })),
            },
        }])
        .build())
}

/// An upstream whose `/hold.webp` answers only after `release` gets permits and whose
/// `/ok.webp` answers at once. `held` counts the requests that reached `/hold.webp`.
async fn holding_upstream(
    release: Arc<tokio::sync::Semaphore>,
    held: Arc<AtomicUsize>,
) -> anyhow::Result<url::Url> {
    let app = axum::Router::new()
        .route(
            "/hold.webp",
            axum::routing::get(move || {
                let (release, held) = (release.clone(), held.clone());
                async move {
                    held.fetch_add(1, Ordering::SeqCst);
                    // Each release permit answers one request, so it is used up, not returned.
                    if let Ok(permit) = release.acquire().await {
                        permit.forget();
                    }
                    "held"
                }
            }),
        )
        .route("/ok.webp", axum::routing::get(|| async { "ok" }));
    common::serve_http_upstream(app).await
}

/// One port behind a trusted proxy on 127.0.0.1 and one port that trusts nobody, both with the
/// background stream limit.
fn limit_storage(
    trusted: &common::TestPort,
    direct: &common::TestPort,
    upstream: &str,
) -> TestStorage {
    let proxy = |client_ip| HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![common::http_route("/", upstream, None)],
        upgrade_insecure: false,
        client_ip,
        stream_priority: StreamPriorityConfig {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let trusting = ClientIpConfig {
        trust_cdn: false,
        trusted_proxies: vec!["127.0.0.0/8".parse().unwrap(), "::1/128".parse().unwrap()],
    };
    TestStorage::builder()
        .ports(vec![
            common::http_port_entry("trusted", trusted),
            common::http_port_entry("direct", direct),
        ])
        .proxies(vec![
            common::http_proxy_entry("behind", "trusted", proxy(trusting)),
            common::http_proxy_entry("open", "direct", proxy(ClientIpConfig::default())),
        ])
        .build()
}

/// Sends `Priority: u=7` with the `X-Forwarded-For` value and returns the status.
async fn background_get(
    client: &reqwest::Client,
    url: url::Url,
    forwarded_for: &str,
) -> anyhow::Result<u16> {
    let resp = client
        .get(url)
        .header("priority", "u=7")
        .header("x-forwarded-for", forwarded_for)
        .send()
        .await?;
    Ok(resp.status().as_u16())
}

/// Starts `count` background requests to `/hold.webp` and waits until the upstream holds them
/// all.
async fn hold_requests(
    client: &reqwest::Client,
    port: &common::TestPort,
    forwarded_for: impl Fn(usize) -> String,
    count: usize,
    held: &AtomicUsize,
) -> anyhow::Result<Vec<tokio::task::JoinHandle<anyhow::Result<u16>>>> {
    let start = held.load(Ordering::SeqCst);
    let tasks = (0..count)
        .map(|i| {
            let (client, url, xff) = (
                client.clone(),
                port.http_url("/hold.webp"),
                forwarded_for(i),
            );
            tokio::spawn(async move { background_get(&client, url, &xff).await })
        })
        .collect();
    let all_held = || async { Ok(held.load(Ordering::SeqCst) == start + count) };
    common::wait_until(all_held, "the upstream did not get every held request").await?;
    Ok(tasks)
}

/// Releases the held requests and asserts that each was answered.
async fn finish(
    tasks: Vec<tokio::task::JoinHandle<anyhow::Result<u16>>>,
    release: &tokio::sync::Semaphore,
) -> anyhow::Result<()> {
    release.add_permits(tasks.len());
    for task in tasks {
        assert_eq!(task.await??, 200);
    }
    Ok(())
}

/// docs/content/configuration.md: the background stream limit counts each client. One client
/// that holds its limit gets 429, while another client is still served; a client that forges
/// `X-Forwarded-For` on a port that trusts nobody stays one client.
#[tokio::test]
async fn background_limit_is_counted_per_client() -> anyhow::Result<()> {
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let held = Arc::new(AtomicUsize::new(0));
    let upstream = holding_upstream(release.clone(), held.clone()).await?;
    let trusted = alloc_tcp_port().await?;
    let direct = alloc_tcp_port().await?;
    let config = limit_storage(&trusted, &direct, upstream.as_str());

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        check_trusted_clients(&client, &trusted, &release, &held).await?;
        check_forged_clients(&client, &direct, &release, &held).await
    })
    .await
}

/// Behind the trusted proxy, the client that holds its limit gets 429 and another client 200.
async fn check_trusted_clients(
    client: &reqwest::Client,
    port: &common::TestPort,
    release: &tokio::sync::Semaphore,
    held: &AtomicUsize,
) -> anyhow::Result<()> {
    let limit = DEFAULT_MAX_BACKGROUND_CONCURRENCY;
    let attacker = "203.0.113.50";
    let tasks = hold_requests(client, port, |_| attacker.into(), limit, held).await?;
    let refused = background_get(client, port.http_url("/ok.webp"), attacker).await?;
    assert_eq!(refused, 429);
    let other = background_get(client, port.http_url("/ok.webp"), "198.51.100.8").await?;
    assert_eq!(other, 200);
    finish(tasks, release).await?;
    let again = background_get(client, port.http_url("/ok.webp"), attacker).await?;
    assert_eq!(again, 200);
    Ok(())
}

/// On the port that trusts nobody every request comes from 127.0.0.1, whatever it forges.
async fn check_forged_clients(
    client: &reqwest::Client,
    port: &common::TestPort,
    release: &tokio::sync::Semaphore,
    held: &AtomicUsize,
) -> anyhow::Result<()> {
    let forged = |i| format!("203.0.113.{i}");
    let tasks = hold_requests(
        client,
        port,
        forged,
        DEFAULT_MAX_BACKGROUND_CONCURRENCY,
        held,
    )
    .await?;
    let refused = background_get(client, port.http_url("/ok.webp"), "198.51.100.9").await?;
    assert_eq!(refused, 429);
    finish(tasks, release).await
}
