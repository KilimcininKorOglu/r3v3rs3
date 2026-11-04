use axum::{
    Router,
    extract::{WebSocketUpgrade, ws::WebSocket},
    response::IntoResponse,
    routing::any,
};
use futures::{SinkExt, StreamExt};
use hyper::Uri;
use r3v3rs3_api::{
    port::{Port, PortEntry},
    proxy::{HttpProxy, Proxy, ProxyEntry, ProxyKind, Route},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

mod common;
use common::{TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, with_server};

#[tokio::test]
async fn ws_proxy() -> anyhow::Result<()> {
    let listen_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;

    let app = Router::new().route("/ws", any(ws_handler));
    let addr = listen_port.socket_addr();
    tokio::spawn(axum_server::bind(addr).serve(app.into_make_service()));

    let config = TestStorage::builder()
        .ports(vec![PortEntry {
            id: "test".parse().unwrap(),
            port: Port {
                active: true,
                name: String::new(),
                listen: proxy_port.multiaddr_http(),
                opts: Default::default(),
            },
            status: None,
        }])
        .proxies(vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            status: None,
            proxy: Proxy {
                ports: vec!["test".parse().unwrap()],
                kind: ProxyKind::Http(Box::new(HttpProxy {
                    vhosts: vec!["localhost".parse().unwrap()],
                    routes: vec![Route {
                        servers: vec![r3v3rs3_api::proxy::Server::new(
                            listen_port.http_url("/").try_into().unwrap(),
                        )],
                        ..Default::default()
                    }],
                    upgrade_insecure: false,
                    client_ip: Default::default(),
                    ip_filter: Default::default(),
                    rate_limit: Default::default(),
                    concurrency_limit: Default::default(),
                    auth: Default::default(),
                    headers: Default::default(),
                    compression: Default::default(),
                    cache: Default::default(),
                    h2c: false,
                    timeouts: Default::default(),
                    load_balancing: Default::default(),
                    health_check: Default::default(),
                    circuit_breaker: Default::default(),
                    retry: Default::default(),
                    sticky: Default::default(),
                    access_list: None,
                    max_body_size: 0,
                    redirects: Vec::new(),
                    client_cert: None,
                    maintenance: Default::default(),
                    error_page_theme: None,
                    error_pages: Vec::new(),
                    bot_blocker: false,
                    grpc_web: false,
                    stream_priority: Default::default(),
                    hide_branding: false,
                })),
                ..Default::default()
            },
        }])
        .build();

    with_server(config, |_| async move {
        let url = Uri::try_from(&format!(
            "ws://localhost:{}/ws",
            proxy_port.socket_addr().port()
        ))?;
        let (mut ws_stream, _) = connect_async(url).await?;
        ws_stream
            .send(Message::Text("Hello, server!".to_string().into()))
            .await?;

        let message = ws_stream.next().await.unwrap();
        match message {
            Ok(msg) => assert_eq!("Hello, server!", msg.into_text()?.as_str()),
            Err(e) => panic!("{e}"),
        }
        Ok(())
    })
    .await
}

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    while let Some(msg) = socket.recv().await {
        if let Ok(msg) = msg {
            socket.send(msg).await.unwrap();
        } else {
            return;
        }
    }
}

#[tokio::test]
async fn non_websocket_upgrade_is_not_tunneled() -> anyhow::Result<()> {
    let listen_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;

    let app = Router::new().route("/h2c", any(|| async { "plain response" }));
    let addr = listen_port.socket_addr();
    tokio::spawn(axum_server::bind(addr).serve(app.into_make_service()));

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("tph2c", &proxy_port)])
        .proxies(vec![http_proxy_entry(
            "tpxh2c",
            "tph2c",
            HttpProxy {
                vhosts: vec!["localhost".parse().unwrap()],
                routes: vec![Route {
                    servers: vec![r3v3rs3_api::proxy::Server::new(
                        listen_port.http_url("/").try_into().unwrap(),
                    )],
                    ..Default::default()
                }],
                ..Default::default()
            },
        )])
        .build();

    with_server(config, |_| async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::TcpStream::connect(proxy_port.socket_addr()).await?;
        let raw = format!(
            "GET /h2c HTTP/1.1\r\nHost: localhost\r\nUpgrade: h2c\r\nConnection: Upgrade, HTTP2-Settings\r\nHTTP2-Settings: AAMAAABkAARAAAAAAAIAAAAA\r\n\r\n"
        );
        stream.write_all(raw.as_bytes()).await?;

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await?;
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 200 OK"), "unexpected response: {resp}");
        assert!(resp.contains("plain response"));
        Ok(())
    })
    .await
}
