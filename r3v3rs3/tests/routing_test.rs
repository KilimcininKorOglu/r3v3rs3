use axum::Router;
use r3v3rs3_api::proxy::{HttpProxy, Route};
use url::Url;

mod common;
use common::{alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server};
use common::{serve_http_upstream, TestPort, TestStorage};

/// Starts an HTTP upstream server that answers every request with its name.
async fn start_named_upstream(name: &'static str) -> anyhow::Result<Url> {
    serve_http_upstream(Router::new().fallback(move || async move { name })).await
}

fn proxy(vhosts: &[&str], routes: Vec<Route>) -> HttpProxy {
    HttpProxy {
        vhosts: vhosts.iter().map(|vhost| vhost.parse().unwrap()).collect(),
        routes,
        ..Default::default()
    }
}

async fn get_name(port: &TestPort, host: &str, path: &str) -> anyhow::Result<String> {
    Ok(reqwest::Client::new()
        .get(port.http_url(path))
        .header("host", host)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

#[tokio::test]
async fn the_most_specific_host_and_the_longest_path_win() -> anyhow::Result<()> {
    let default = start_named_upstream("default").await?;
    let api = start_named_upstream("api").await?;
    let app = start_named_upstream("app").await?;
    let wildcard = start_named_upstream("wildcard").await?;
    let port = alloc_tcp_port().await?;

    // Every less specific route and proxy comes first, so the first match would be wrong.
    let any_host = proxy(
        &[],
        vec![
            http_route("/", default.as_str(), None),
            http_route("/api", api.as_str(), None),
        ],
    );
    let wildcard_host = proxy(
        &["*.example.test"],
        vec![http_route("/", wildcard.as_str(), None)],
    );
    let exact_host = proxy(
        &["app.example.test"],
        vec![http_route("/", app.as_str(), None)],
    );
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("route", &port)])
        .proxies(vec![
            http_proxy_entry("any", "route", any_host),
            http_proxy_entry("wild", "route", wildcard_host),
            http_proxy_entry("exact", "route", exact_host),
        ])
        .build();

    with_server(config, |_| async move {
        assert_eq!(get_name(&port, "other.test", "/api/users").await?, "api");
        assert_eq!(get_name(&port, "other.test", "/about").await?, "default");
        assert_eq!(get_name(&port, "x.example.test", "/api").await?, "wildcard");
        assert_eq!(get_name(&port, "app.example.test", "/api").await?, "app");
        Ok(())
    })
    .await
}
