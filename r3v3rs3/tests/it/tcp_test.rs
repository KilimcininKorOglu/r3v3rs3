use crate::common;
use axum::{Router, routing::get};
use common::{
    TestPort, TestStorage, admin_session_cookie, alloc_tcp_port, port_entry, wait_for_listener,
    with_server,
};
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::{
    port::UpstreamServer,
    proxy::{Proxy, ProxyEntry, ProxyKind, TcpProxy},
};
use reqwest::header::COOKIE;
use std::collections::HashMap;
use tracing_subscriber::filter::LevelFilter;

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
            source: None,
            status: None,
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

    spawn_hello_upstreams(&first_port, &second_port);
    let storage = updatable_storage(&proxy_port, &first_port);

    let app_info = new_appinfo(&dir, &dir);
    with_server(storage, |channels| async move {
        tokio::spawn(start_admin(
            app_info,
            admin_addr,
            channels.command,
            channels.callback,
            channels.event.clone(),
            channels.accounts.clone(),
        ));
        wait_for_listener(admin_addr).await?;
        check_updated_upstream(&proxy_port, admin_addr, &second_port).await
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

/// Starts the two upstreams that answer `/hello` with `first` and `second`.
fn spawn_hello_upstreams(first_port: &TestPort, second_port: &TestPort) {
    for (port, body) in [(first_port, "first"), (second_port, "second")] {
        let app = Router::new().route("/hello", get(move || async move { body }));
        tokio::spawn(axum_server::bind(port.socket_addr()).serve(app.into_make_service()));
    }
}

/// Builds the storage with a TCP proxy to the first upstream and an admin account.
fn updatable_storage(proxy_port: &TestPort, first_port: &TestPort) -> TestStorage {
    TestStorage::builder()
        .ports(vec![port_entry("test", proxy_port.multiaddr_tcp())])
        .proxies(vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            status: None,
            proxy: tcp_proxy_to(first_port),
        }])
        .accounts(HashMap::from([("admin".to_string(), "secret".to_string())]))
        .build()
}

/// Fetches `/hello` through the proxy with a new client, so each request opens a new proxied
/// connection.
async fn fetch_hello(proxy_port: &TestPort) -> anyhow::Result<String> {
    let resp = reqwest::get(proxy_port.http_url("/hello")).await?;
    Ok(resp.text().await?)
}

/// Checks that a proxy update through the admin API moves the new connections to the new
/// upstream.
async fn check_updated_upstream(
    proxy_port: &TestPort,
    admin_addr: std::net::SocketAddr,
    second_port: &TestPort,
) -> anyhow::Result<()> {
    // reqwest::get uses a new client, so each request opens a new proxied connection.
    assert_eq!(fetch_hello(proxy_port).await?, "first");

    let cookie = admin_session_cookie(admin_addr).await?;
    reqwest::Client::new()
        .put(format!("http://{admin_addr}/api/proxies/test2"))
        .header(COOKIE, cookie)
        .json(&tcp_proxy_to(second_port))
        .send()
        .await?
        .error_for_status()?;

    assert_eq!(fetch_hello(proxy_port).await?, "second");
    Ok(())
}

/// Accepts connections and holds each one open until the client closes it.
fn spawn_holding_upstream(port: &TestPort) {
    let addr = port.socket_addr();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        loop {
            let (mut socket, _) = listener.accept().await?;
            tokio::spawn(async move {
                let mut sink = tokio::io::sink();
                tokio::io::copy(&mut socket, &mut sink).await
            });
        }
        #[allow(unreachable_code)]
        anyhow::Ok(())
    });
}

/// The ids of the tracked connections whose client address is one of `clients`.
fn tracked_ids(clients: &[String]) -> Vec<r3v3rs3_api::id::ShortId> {
    r3v3rs3::proxy::connection_tracker::list(None)
        .into_iter()
        .filter(|conn| clients.contains(&conn.client_addr))
        .map(|conn| conn.id)
        .collect()
}

/// Two open connections are two entries of the active connection list, each with its own id,
/// because the panel terminates a connection by its id (an id shared by two connections would
/// close or hide the wrong one).
#[tokio::test]
async fn each_open_tcp_connection_has_its_own_tracked_id() -> anyhow::Result<()> {
    let upstream_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;
    spawn_holding_upstream(&upstream_port);
    let config = TestStorage::builder()
        .ports(vec![port_entry("test", proxy_port.multiaddr_tcp())])
        .proxies(vec![ProxyEntry {
            id: "tracked".parse()?,
            source: None,
            status: None,
            proxy: tcp_proxy_to(&upstream_port),
        }])
        .build();

    with_server(config, |_| check_two_tracked_ids(proxy_port)).await
}

/// Opens two connections through the proxy and checks that each has its own tracked id.
async fn check_two_tracked_ids(proxy_port: TestPort) -> anyhow::Result<()> {
    wait_for_listener(proxy_port.socket_addr()).await?;
    let first = tokio::net::TcpStream::connect(proxy_port.socket_addr()).await?;
    let second = tokio::net::TcpStream::connect(proxy_port.socket_addr()).await?;
    let clients = [
        first.local_addr()?.to_string(),
        second.local_addr()?.to_string(),
    ];
    let clients_ref = &clients;
    common::wait_until(
        || async move { Ok(tracked_ids(clients_ref).len() == 2) },
        "the two open connections are not two tracked entries",
    )
    .await?;
    let ids = tracked_ids(&clients);
    assert_ne!(ids[0], ids[1], "two open connections share one id");
    drop((first, second));
    Ok(())
}

fn tcp_proxy_to(upstream: &TestPort) -> Proxy {
    Proxy {
        ports: vec!["test".parse().unwrap()],
        kind: ProxyKind::Tcp(TcpProxy {
            client_cert: None,
            connect_timeout: r3v3rs3_api::upstream::DEFAULT_CONNECT_TIMEOUT,
            load_balancing: Default::default(),
            health_check: Default::default(),
            circuit_breaker: Default::default(),
            proxy_protocol: None,
            upstream_servers: vec![UpstreamServer::new(upstream.multiaddr_tcp())],
        }),
        ..Default::default()
    }
}
