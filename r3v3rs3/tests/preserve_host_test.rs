use r3v3rs3_api::proxy::HttpProxy;
use reqwest::StatusCode;

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

/// Sends a request for `localhost` through a proxy and checks that the upstream server receives
/// the `Host` header that `expected_host` returns for its address.
async fn upstream_host(
    preserve_host: bool,
    expected_host: fn(&str) -> String,
) -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/")
        .match_header("host", expected_host(&server.host_with_port()).as_str())
        .with_status(200)
        .with_body("ok")
        .create_async()
        .await;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &server.url(), None)],
        preserve_host,
        ..Default::default()
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("hst-test", "http", proxy)])
        .build();

    with_server(config, |_| async move {
        let res = reqwest::Client::new()
            .get(port.http_url("/"))
            .header("Host", "localhost")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        Ok(())
    })
    .await?;
    mock.assert_async().await;
    Ok(())
}

#[tokio::test]
async fn the_upstream_receives_the_client_host_when_it_is_kept() -> anyhow::Result<()> {
    upstream_host(true, |_| "localhost".into()).await
}

#[tokio::test]
async fn the_upstream_receives_its_own_address_by_default() -> anyhow::Result<()> {
    upstream_host(false, str::to_string).await
}
