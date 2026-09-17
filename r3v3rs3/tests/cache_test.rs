use mockito::Matcher;
use r3v3rs3::{
    command::ServerCommand,
    server::rpc::{ErasedRpcMethod, RpcWrapper, proxies::PurgeProxyCache},
};
use r3v3rs3_api::{cache::CacheConfig, proxy::HttpProxy};
use reqwest::{
    StatusCode,
    header::{AGE, IF_NONE_MATCH},
};

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
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
