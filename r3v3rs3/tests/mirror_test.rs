use axum::http::Uri;
use axum::Router;
use r3v3rs3_api::mirror::Mirror;
use r3v3rs3_api::proxy::{HttpProxy, Route, Server};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use url::Url;

mod common;
use common::{alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server};
use common::{serve_http_upstream, TestStorage};

type Copies = mpsc::UnboundedReceiver<(String, String)>;

/// Starts a mirror server that reports the path, the query and the body of each copy, and answers
/// after 2 seconds.
async fn start_slow_mirror() -> anyhow::Result<(Url, Copies)> {
    let (sender, receiver) = mpsc::unbounded_channel();
    let app = Router::new().fallback(move |uri: Uri, body: String| {
        let sender = sender.clone();
        async move {
            let path = uri
                .path_and_query()
                .map(ToString::to_string)
                .unwrap_or_default();
            let _ = sender.send((path, body));
            tokio::time::sleep(Duration::from_secs(2)).await;
            "mirror"
        }
    });
    Ok((serve_http_upstream(app).await?, receiver))
}

async fn next_copy(copies: &mut Copies) -> anyhow::Result<Option<(String, String)>> {
    Ok(tokio::time::timeout(Duration::from_secs(5), copies.recv()).await?)
}

#[tokio::test]
async fn requests_are_copied_without_waiting_for_the_mirror() -> anyhow::Result<()> {
    let echo =
        serve_http_upstream(Router::new().fallback(|body: String| async move { body })).await?;
    let (shadow, mut copies) = start_slow_mirror().await?;
    let port = alloc_tcp_port().await?;
    let route = Route {
        mirror: Some(Mirror {
            servers: vec![Server::new(shadow.join("copy/")?.as_str().parse()?)],
            percent: 100,
            max_body_size: 8,
        }),
        ..http_route("/api", echo.as_str(), None)
    };
    let proxy = HttpProxy {
        routes: vec![route],
        ..Default::default()
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("mirror", &port)])
        .proxies(vec![http_proxy_entry("mirrorp", "mirror", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        let started = Instant::now();
        let res = client
            .post(port.http_url("/api/items?q=1"))
            .body("hello")
            .send()
            .await?;
        assert_eq!(res.text().await?, "hello");
        assert!(started.elapsed() < Duration::from_secs(1));
        let copy = ("/copy/items?q=1".to_string(), "hello".to_string());
        assert_eq!(next_copy(&mut copies).await?, Some(copy));

        // The body is longer than the copy limit, so only the next request reaches the mirror.
        let res = client
            .post(port.http_url("/api/large"))
            .body("0123456789")
            .send()
            .await?;
        assert_eq!(res.text().await?, "0123456789");
        let res = client.get(port.http_url("/api/empty")).send().await?;
        assert_eq!(res.status(), 200);
        let copy = ("/copy/empty".to_string(), String::new());
        assert_eq!(next_copy(&mut copies).await?, Some(copy));
        Ok(())
    })
    .await
}
