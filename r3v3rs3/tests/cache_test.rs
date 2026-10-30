use axum::{Router, routing::get};
use mockito::Matcher;
use r3v3rs3::{
    command::ServerCommand,
    server::{
        ServerChannels,
        rpc::{ErasedRpcMethod, RpcWrapper, proxies::PurgeProxyCache},
    },
};
use r3v3rs3_api::{cache::CacheConfig, id::ShortId, proxy::HttpProxy};
use reqwest::{
    StatusCode,
    header::{AGE, IF_NONE_MATCH},
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

mod common;
use common::{
    TestStorage, alloc_tcp_port, call, http_port_entry, http_proxy_entry, http_route,
    serve_http_upstream, with_server,
};

fn x_cache(resp: &reqwest::Response) -> Option<&str> {
    resp.headers()
        .get("x-cache")
        .and_then(|value| value.to_str().ok())
}

#[tokio::test]
async fn cache_serves_fresh_responses_and_revalidates_stale_ones() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut upstream = mockito::Server::new_async().await;

    let mock_fresh = upstream
        .mock("GET", "/fresh")
        .match_header("accept-encoding", Matcher::Missing)
        .with_header("content-type", "text/plain")
        .with_header("cache-control", "max-age=60")
        .with_body("fresh")
        .expect(2)
        .create_async()
        .await;
    let mock_no_store = upstream
        .mock("GET", "/no-store")
        .with_header("cache-control", "no-store")
        .with_body("no-store")
        .expect(2)
        .create_async()
        .await;
    let mock_cookie = upstream
        .mock("GET", "/cookie")
        .with_header("cache-control", "max-age=60")
        .with_header("set-cookie", "id=1")
        .with_body("cookie")
        .expect(2)
        .create_async()
        .await;
    let mock_en = upstream
        .mock("GET", "/vary")
        .match_header("x-lang", "en")
        .with_header("cache-control", "max-age=60")
        .with_header("vary", "X-Lang")
        .with_body("en")
        .expect(1)
        .create_async()
        .await;
    let mock_tr = upstream
        .mock("GET", "/vary")
        .match_header("x-lang", "tr")
        .with_header("cache-control", "max-age=60")
        .with_header("vary", "X-Lang")
        .with_body("tr")
        .expect(1)
        .create_async()
        .await;
    let mock_stale = upstream
        .mock("GET", "/stale")
        .match_header("if-none-match", Matcher::Missing)
        .with_header("cache-control", "no-cache")
        .with_header("etag", "\"v1\"")
        .with_body("stale body")
        .expect(1)
        .create_async()
        .await;
    let mock_revalidate = upstream
        .mock("GET", "/stale")
        .match_header("if-none-match", "\"v1\"")
        .with_status(304)
        .with_header("cache-control", "no-cache")
        .with_header("etag", "\"v1\"")
        .expect(2)
        .create_async()
        .await;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream.url(), None)],
        upgrade_insecure: false,
        cache: CacheConfig {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("cache", &port)])
        .proxies(vec![http_proxy_entry("proxy1", "cache", proxy)])
        .build();

    with_server(config, |mut channels| async move {
        let client = reqwest::Client::new();
        let get = |path: &'static str| client.get(port.http_url(path));

        let resp = get("/fresh").send().await?;
        assert_eq!(x_cache(&resp), Some("MISS"));
        assert_eq!(resp.text().await?, "fresh");
        let resp = get("/fresh").send().await?;
        assert_eq!(x_cache(&resp), Some("HIT"));
        assert!(resp.headers().contains_key(AGE));
        assert_eq!(resp.text().await?, "fresh");
        let resp = client.head(port.http_url("/fresh")).send().await?;
        assert_eq!(x_cache(&resp), Some("HIT"));
        assert!(resp.bytes().await?.is_empty());

        for path in ["/no-store", "/cookie", "/no-store", "/cookie"] {
            let resp = get(path).send().await?;
            assert_eq!(x_cache(&resp), Some("MISS"), "{path}");
        }

        let resp = get("/vary").header("x-lang", "en").send().await?;
        assert_eq!(x_cache(&resp), Some("MISS"));
        let resp = get("/vary").header("x-lang", "en").send().await?;
        assert_eq!(x_cache(&resp), Some("HIT"));
        assert_eq!(resp.text().await?, "en");
        let resp = get("/vary").header("x-lang", "tr").send().await?;
        assert_eq!(x_cache(&resp), Some("MISS"));
        assert_eq!(resp.text().await?, "tr");

        let resp = get("/stale").send().await?;
        assert_eq!(x_cache(&resp), Some("MISS"));
        assert_eq!(resp.text().await?, "stale body");
        let resp = get("/stale").send().await?;
        assert_eq!(x_cache(&resp), Some("HIT"));
        assert_eq!(resp.text().await?, "stale body");
        let resp = get("/stale").header(IF_NONE_MATCH, "\"v1\"").send().await?;
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(x_cache(&resp), Some("HIT"));

        let arg = Box::new(RpcWrapper::new(PurgeProxyCache {
            id: "proxy1".parse()?,
            path: None,
        })) as Box<dyn ErasedRpcMethod>;
        channels
            .command
            .send(ServerCommand::CallMethod {
                id: 1,
                arg,
                caller: r3v3rs3::accounts::Caller::system(),
            })
            .await?;
        let callback = channels
            .callback
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
        assert!(callback.result.is_ok());

        let resp = get("/fresh").send().await?;
        assert_eq!(x_cache(&resp), Some("MISS"));
        assert_eq!(resp.text().await?, "fresh");

        Ok(())
    })
    .await?;

    for mock in [
        mock_fresh,
        mock_no_store,
        mock_cookie,
        mock_en,
        mock_tr,
        mock_stale,
        mock_revalidate,
    ] {
        mock.assert_async().await;
    }
    Ok(())
}

async fn check_swr(client: &reqwest::Client, base_url: &url::Url) -> anyhow::Result<()> {
    let url = base_url.join("/swr")?;
    let resp = client.get(url.clone()).send().await?;
    assert_eq!(x_cache(&resp), Some("MISS"));
    assert_eq!(resp.text().await?, "count 0");

    let resp = client.get(url.clone()).send().await?;
    assert_eq!(x_cache(&resp), Some("HIT"));
    assert_eq!(resp.text().await?, "count 0");

    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    // Serves stale immediately while triggering background revalidation
    let resp = client.get(url.clone()).send().await?;
    assert_eq!(x_cache(&resp), Some("STALE"));
    assert_eq!(resp.text().await?, "count 0");

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let resp = client.get(url).send().await?;
    assert_eq!(x_cache(&resp), Some("HIT"));
    assert_eq!(resp.text().await?, "count 1");
    Ok(())
}

async fn check_purge_method(client: &reqwest::Client, base_url: &url::Url) -> anyhow::Result<()> {
    let swr_url = base_url.join("/swr")?;
    let purge_resp = client
        .request(
            reqwest::Method::from_bytes(b"PURGE").unwrap(),
            swr_url.clone(),
        )
        .send()
        .await?;
    assert_eq!(purge_resp.status(), StatusCode::OK);
    assert!(purge_resp.text().await?.contains("Purged"));

    let resp = client.get(swr_url).send().await?;
    assert_eq!(x_cache(&resp), Some("MISS"));
    assert_eq!(resp.text().await?, "count 2");
    Ok(())
}

fn swr_router(counter: Arc<AtomicUsize>) -> Router {
    Router::new()
        .route(
            "/swr",
            get(move || {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                async move {
                    (
                        [
                            ("cache-control", "max-age=1, stale-while-revalidate=5"),
                            ("content-type", "text/plain"),
                        ],
                        format!("count {n}"),
                    )
                }
            }),
        )
        .route(
            "/item1",
            get(|| async {
                (
                    [
                        ("cache-control", "max-age=60"),
                        ("content-type", "text/plain"),
                    ],
                    "item1",
                )
            }),
        )
        .route(
            "/item2",
            get(|| async {
                (
                    [
                        ("cache-control", "max-age=60"),
                        ("content-type", "text/plain"),
                    ],
                    "item2",
                )
            }),
        )
}

async fn assert_cached_items(
    client: &reqwest::Client,
    url1: &url::Url,
    url2: &url::Url,
) -> anyhow::Result<()> {
    let _ = client.get(url1.clone()).send().await?;
    let r1 = client.get(url1.clone()).send().await?;
    assert_eq!(x_cache(&r1), Some("HIT"));

    let _ = client.get(url2.clone()).send().await?;
    let r2 = client.get(url2.clone()).send().await?;
    assert_eq!(x_cache(&r2), Some("HIT"));
    Ok(())
}

async fn check_purge_rpc(
    client: &reqwest::Client,
    base_url: &url::Url,
    channels: &mut ServerChannels,
    proxy_id: ShortId,
) -> anyhow::Result<()> {
    let url1 = base_url.join("/item1")?;
    let url2 = base_url.join("/item2")?;
    assert_cached_items(client, &url1, &url2).await?;

    call(
        channels,
        PurgeProxyCache {
            id: proxy_id,
            path: Some("/item1".into()),
        },
    )
    .await??;

    let r1 = client.get(url1).send().await?;
    assert_eq!(x_cache(&r1), Some("MISS"));
    let r2 = client.get(url2).send().await?;
    assert_eq!(x_cache(&r2), Some("HIT"));
    Ok(())
}

#[tokio::test]
async fn stale_while_revalidate_and_purge() -> anyhow::Result<()> {
    let counter = Arc::new(AtomicUsize::new(0));
    let upstream = serve_http_upstream(swr_router(counter)).await?;
    let port = alloc_tcp_port().await?;
    let proxy_id: ShortId = "pxswr".parse()?;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse()?],
        routes: vec![http_route("/", upstream.as_str(), None)],
        cache: CacheConfig {
            enabled: true,
            stale_while_revalidate: std::time::Duration::from_secs(5),
            ..Default::default()
        },
        ..Default::default()
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("p_swr", &port)])
        .proxies(vec![http_proxy_entry("pxswr", "p_swr", proxy)])
        .build();

    with_server(config, |mut channels| async move {
        let client = reqwest::Client::new();
        let base_url = port.http_url("/");
        check_swr(&client, &base_url).await?;
        check_purge_method(&client, &base_url).await?;
        check_purge_rpc(&client, &base_url, &mut channels, proxy_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn revalidation_304_does_not_cache_set_cookie_or_private() -> anyhow::Result<()> {
    let mut upstream = mockito::Server::new_async().await;
    let port = alloc_tcp_port().await?;

    let mock_stale = upstream
        .mock("GET", "/session-stale")
        .match_header("if-none-match", Matcher::Missing)
        .with_header("cache-control", "no-cache")
        .with_header("etag", "\"v1\"")
        .with_body("session body")
        .expect(1)
        .create_async()
        .await;

    let mock_revalidate_cookie = upstream
        .mock("GET", "/session-stale")
        .match_header("if-none-match", "\"v1\"")
        .with_status(304)
        .with_header("cache-control", "no-cache")
        .with_header("set-cookie", "session=secret123; Path=/")
        .with_header("etag", "\"v1\"")
        .expect(1)
        .create_async()
        .await;

    let mock_revalidate_next = upstream
        .mock("GET", "/session-stale")
        .match_header("if-none-match", "\"v1\"")
        .with_status(304)
        .with_header("cache-control", "no-cache")
        .with_header("etag", "\"v1\"")
        .expect(1)
        .create_async()
        .await;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream.url(), None)],
        upgrade_insecure: false,
        cache: CacheConfig {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("cache", &port)])
        .proxies(vec![http_proxy_entry("proxy1", "cache", proxy)])
        .build();

    with_server(config, |_channels| async move {
        let client = reqwest::Client::new();
        let get = |path: &'static str| client.get(port.http_url(path));

        let resp1 = get("/session-stale").send().await?;
        assert_eq!(x_cache(&resp1), Some("MISS"));
        assert_eq!(resp1.text().await?, "session body");

        let resp2 = get("/session-stale").send().await?;
        assert_eq!(
            resp2
                .headers()
                .get("set-cookie")
                .and_then(|v| v.to_str().ok()),
            Some("session=secret123; Path=/")
        );
        assert_eq!(resp2.text().await?, "session body");

        let resp3 = get("/session-stale").send().await?;
        assert!(
            !resp3.headers().contains_key("set-cookie"),
            "Client B received leaked Set-Cookie from Client A: {:?}",
            resp3.headers().get("set-cookie")
        );
        assert_eq!(resp3.text().await?, "session body");

        Ok(())
    })
    .await?;

    mock_stale.assert_async().await;
    mock_revalidate_cookie.assert_async().await;
    mock_revalidate_next.assert_async().await;
    Ok(())
}

#[tokio::test]
async fn cookie_authenticated_requests_are_not_cached_without_public() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut upstream = mockito::Server::new_async().await;

    let mock_profile = upstream
        .mock("GET", "/profile")
        .with_header("content-type", "text/plain")
        .with_header("cache-control", "max-age=60")
        .with_body("user private profile")
        .expect(2)
        .create_async()
        .await;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream.url(), None)],
        upgrade_insecure: false,
        cache: CacheConfig {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("cache", &port)])
        .proxies(vec![http_proxy_entry("proxy1", "cache", proxy)])
        .build();

    with_server(config, |_channels| async move {
        let client = reqwest::Client::new();
        let base_url = port.http_url("/profile");

        let resp1 = client
            .get(base_url.clone())
            .header("cookie", "session_user_a=secret1")
            .send()
            .await?;
        assert_eq!(resp1.status(), StatusCode::OK);
        assert_eq!(resp1.text().await?, "user private profile");

        let resp2 = client
            .get(base_url)
            .header("cookie", "session_user_b=secret2")
            .send()
            .await?;
        assert_eq!(resp2.status(), StatusCode::OK);
        assert_ne!(x_cache(&resp2), Some("HIT"));
        assert_eq!(resp2.text().await?, "user private profile");

        mock_profile.assert_async().await;
        Ok(())
    })
    .await
}
