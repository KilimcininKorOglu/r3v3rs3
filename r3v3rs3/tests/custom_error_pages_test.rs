use axum::http::StatusCode;
use r3v3rs3_api::proxy::{CustomErrorPage, HttpProxy, Route};
use url::Url;

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route,
    serve_http_upstream, with_server,
};

fn proxy_with_error_pages(routes: Vec<Route>, error_pages: Vec<CustomErrorPage>) -> HttpProxy {
    HttpProxy {
        routes,
        error_pages,
        ..Default::default()
    }
}

#[tokio::test]
async fn test_custom_error_pages_for_404_and_502() -> anyhow::Result<()> {
    let not_found_upstream =
        serve_http_upstream(axum::Router::new().fallback(|| async { StatusCode::NOT_FOUND }))
            .await?;
    let ok_upstream =
        serve_http_upstream(axum::Router::new().fallback(|| async { "ok upstream" })).await?;

    let port = alloc_tcp_port().await?;
    let broken_upstream: Url = "http://127.0.0.1:1".parse()?;

    let custom_404 = "<html><body>Custom 404 Not Found</body></html>";
    let custom_502 = "<html><body>Custom 502 Bad Gateway</body></html>";

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

    with_server(storage, |_| async move {
        let client = reqwest::Client::new();

        // 1. Upstream returns 404 -> rewritten with custom 404 HTML
        let res_404 = client.get(port.http_url("/missing")).send().await?;
        assert_eq!(res_404.status(), 404);
        assert_eq!(
            res_404.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        assert_eq!(res_404.text().await?, custom_404);

        // 2. Unreachable upstream -> gateway error rewritten with custom 502 HTML
        let res_502 = client.get(port.http_url("/broken")).send().await?;
        assert_eq!(res_502.status(), 502);
        assert_eq!(
            res_502.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        assert_eq!(res_502.text().await?, custom_502);

        // 3. Normal 200 response -> not rewritten
        let res_200 = client.get(port.http_url("/ok")).send().await?;
        assert_eq!(res_200.status(), 200);
        assert_eq!(res_200.text().await?, "ok upstream");

        // 4. No route found (502 Bad Gateway) -> rewritten with custom 502 HTML
        let res_no_route = client.get(port.http_url("/unknown")).send().await?;
        assert_eq!(res_no_route.status(), 502);
        assert_eq!(
            res_no_route.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        assert_eq!(res_no_route.text().await?, custom_502);

        Ok(())
    })
    .await
}
