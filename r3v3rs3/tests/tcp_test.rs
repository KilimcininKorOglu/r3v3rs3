use axum::{routing::get, Router};
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::{
    port::UpstreamServer,
    proxy::{Proxy, ProxyEntry, ProxyKind, TcpProxy},
};
use reqwest::header::COOKIE;
use std::collections::HashMap;
use tracing_subscriber::filter::LevelFilter;
mod common;
use common::{
    admin_session_cookie, alloc_tcp_port, port_entry, wait_for_listener, with_server, TestPort,
    TestStorage,
};

#[tokio::test]
async fn tcp_proxy() -> anyhow::Result<()> {
    let listen_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;

    async fn handler() -> &'static str {
        "Hello"
    }
    let app = Router::new().route("/hello", get(handler));

    let addr = listen_port.socket_addr();
    tokio::spawn(axum_server::bind(addr).serve(app.into_make_service()));

    let config = TestStorage::builder()
        .ports(vec![port_entry("test", proxy_port.multiaddr_tcp())])
        .proxies(vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            proxy: tcp_proxy_to(&listen_port),
        }])
        .build();

    with_server(config, |_| async move {
        let resp = reqwest::get(proxy_port.http_url("/hello"))
            .await?
            .text()
            .await?;
        assert_eq!(resp, "Hello");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn tcp_proxy_uses_updated_upstream() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-tcp-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;

    let first_port = alloc_tcp_port().await?;
    let second_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;
    let admin_addr = alloc_tcp_port().await?.socket_addr();

    for (port, body) in [(&first_port, "first"), (&second_port, "second")] {
        let app = Router::new().route("/hello", get(move || async move { body }));
        tokio::spawn(axum_server::bind(port.socket_addr()).serve(app.into_make_service()));
    }

    let storage = TestStorage::builder()
        .ports(vec![port_entry("test", proxy_port.multiaddr_tcp())])
        .proxies(vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            proxy: tcp_proxy_to(&first_port),
        }])
        .accounts(HashMap::from([("admin".to_string(), "secret".to_string())]))
        .build();

    let app_info = new_appinfo(&dir, &dir);
    with_server(storage, |channels| async move {
        tokio::spawn(start_admin(
            app_info,
            admin_addr,
            channels.command,
            channels.callback,
            channels.event.clone(),
        ));
        wait_for_listener(admin_addr).await?;

        // reqwest::get uses a new client, so each request opens a new proxied connection.
        let resp = reqwest::get(proxy_port.http_url("/hello")).await?;
        assert_eq!(resp.text().await?, "first");

        let cookie = admin_session_cookie(admin_addr).await?;
        reqwest::Client::new()
            .put(format!("http://{admin_addr}/api/proxies/test2"))
            .header(COOKIE, cookie)
            .json(&tcp_proxy_to(&second_port))
            .send()
            .await?
            .error_for_status()?;

        let resp = reqwest::get(proxy_port.http_url("/hello")).await?;
        assert_eq!(resp.text().await?, "second");
        Ok(())
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

fn tcp_proxy_to(upstream: &TestPort) -> Proxy {
    Proxy {
        ports: vec!["test".parse().unwrap()],
        kind: ProxyKind::Tcp(TcpProxy {
            client_cert: None,
            upstream_servers: vec![UpstreamServer {
                addr: upstream.multiaddr_tcp(),
            }],
        }),
        ..Default::default()
    }
}
