use axum::http::StatusCode;
use r3v3rs3_api::proxy::{CustomErrorPage, HttpProxy, Route};
use url::Url;

mod common;
use common::{
    TestPort, TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route,
    serve_http_upstream, with_server,
};

async fn assert_custom_rewritten(
    client: &reqwest::Client,
    url: Url,
    expected_status: u16,
    expected_body: &str,
) -> anyhow::Result<()> {
    let res = client.get(url).send().await?;
    assert_eq!(res.status(), expected_status);
    assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(res.text().await?, expected_body);
    Ok(())
}

fn proxy_with_error_pages(routes: Vec<Route>, error_pages: Vec<CustomErrorPage>) -> HttpProxy {
    HttpProxy {
        routes,
        error_pages,
        ..Default::default()
    }
}

async fn create_test_fixture(
    custom_404: &str,
    custom_502: &str,
) -> anyhow::Result<(TestPort, TestStorage)> {
    let not_found_upstream =
        serve_http_upstream(axum::Router::new().fallback(|| async { StatusCode::NOT_FOUND }))
            .await?;
    let ok_upstream =
        serve_http_upstream(axum::Router::new().fallback(|| async { "ok upstream" })).await?;

    let port = alloc_tcp_port().await?;
    let broken_upstream: Url = "http://127.0.0.1:1".parse()?;

    let proxy = proxy_with_error_pages(
        vec![
            http_route("/missing", not_found_upstream.as_str(), None),
            http_route("/broken", broken_upstream.as_str(), None),
            http_route("/ok", ok_upstream.as_str(), None),
        ],
        vec![
            CustomErrorPage {
                status: 404,
                body: custom_404.to_string(),
            },
            CustomErrorPage {
                status: 502,
                body: custom_502.to_string(),
            },
        ],
    );

    let storage = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("test", "http", proxy)])
        .build();

    Ok((port, storage))
}

#[tokio::test]
async fn test_custom_error_pages_for_404_and_502() -> anyhow::Result<()> {
    let custom_404 = "<html><body>Custom 404 Not Found</body></html>";
    let custom_502 = "<html><body>Custom 502 Bad Gateway</body></html>";
    let (port, storage) = create_test_fixture(custom_404, custom_502).await?;

    with_server(storage, |_| async move {
        let client = reqwest::Client::new();
        assert_custom_rewritten(&client, port.http_url("/missing"), 404, custom_404).await?;
        assert_custom_rewritten(&client, port.http_url("/broken"), 502, custom_502).await?;

        let res_200 = client.get(port.http_url("/ok")).send().await?;
        assert_eq!(res_200.status(), 200);
        assert_eq!(res_200.text().await?, "ok upstream");

        assert_custom_rewritten(&client, port.http_url("/unknown"), 502, custom_502).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_custom_error_page_escapes_host_xss() -> anyhow::Result<()> {
    let not_found_upstream =
        serve_http_upstream(axum::Router::new().fallback(|| async { StatusCode::NOT_FOUND }))
            .await?;
    let port = alloc_tcp_port().await?;
    let custom_404 = "<html><body><h1>Error on {{host}}</h1><p>{{message}}</p></body></html>";
    let proxy = proxy_with_error_pages(
        vec![http_route("/missing", not_found_upstream.as_str(), None)],
        vec![CustomErrorPage {
            status: 404,
            body: custom_404.to_string(),
        }],
    );
    let storage = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("test", "http", proxy)])
        .build();

    with_server(storage, |_| async move {
        let client = reqwest::Client::new();
        let xss_host = "target.local\"><script>alert('xss')</script>";
        let res = client
            .get(port.http_url("/missing"))
            .header("Host", xss_host)
            .send()
            .await?;
        assert_eq!(res.status(), 404);
        let text = res.text().await?;
        assert!(!text.contains("<script>"));
        assert!(text.contains("&lt;script&gt;alert(&#x27;xss&#x27;)&lt;/script&gt;"));
        Ok(())
    })
    .await
}
