//! The proxy hands a request for the address of a registered panel to the panel router in
//! process. No server listens on that address, so every answer below proves the in-process path.

use axum::{
    Router,
    extract::{ConnectInfo, WebSocketUpgrade, ws::WebSocket},
    http::HeaderMap,
    response::{
        IntoResponse, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{any, get},
};
use futures::{SinkExt, StreamExt};
use hyper::Uri;
use r3v3rs3::admin::panel;
use r3v3rs3_api::proxy::HttpProxy;
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};

mod common;
use common::{TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route};

/// Answers with the peer address and the `Host` that the panel sees.
async fn whoami(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> String {
    let host = headers
        .get("host")
        .and_then(|host| host.to_str().ok())
        .unwrap_or_default();
    format!("{} {host}", peer.ip())
}

async fn echo(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(|mut socket: WebSocket| async move {
        while let Some(Ok(message)) = socket.recv().await {
            if socket.send(message).await.is_err() {
                return;
            }
        }
    })
}

/// Sends one event and keeps the stream open, as the event stream of the panel does.
async fn events() -> impl IntoResponse {
    let first =
        futures::stream::once(async { Ok::<_, axum::Error>(Event::default().data("ready")) });
    Sse::new(first.chain(futures::stream::pending())).keep_alive(KeepAlive::default())
}

#[tokio::test]
async fn the_proxy_serves_a_registered_panel_in_process() -> anyhow::Result<()> {
    let panel_addr = alloc_tcp_port().await?.socket_addr();
    let proxy_port = alloc_tcp_port().await?;
    let router = Router::new()
        .route("/whoami", get(whoami))
        .route("/ws", any(echo))
        .route("/events", get(events));
    panel::install(panel_addr, router);

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("pnl-prt", &proxy_port)])
        .proxies(vec![http_proxy_entry(
            "pnl-prx",
            "pnl-prt",
            HttpProxy {
                vhosts: vec!["localhost".parse().unwrap()],
                routes: vec![http_route("/", &format!("http://{panel_addr}/"), None)],
                ..Default::default()
            },
        )])
        .build();

    let port = proxy_port.socket_addr().port();
    let result = common::with_server(config, |_| async move {
        check_whoami(port).await?;
        check_websocket_echo(port).await?;
        check_event_stream(port).await
    })
    .await;
    panel::clear(panel_addr);
    result
}

/// Checks that the panel sees the loopback peer and the `Host` of the client.
async fn check_whoami(port: u16) -> anyhow::Result<()> {
    let whoami = reqwest::get(format!("http://localhost:{port}/whoami"))
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(whoami, format!("127.0.0.1 localhost:{port}"));
    Ok(())
}

/// Checks that a WebSocket message comes back from the panel through the proxy.
async fn check_websocket_echo(port: u16) -> anyhow::Result<()> {
    let url = Uri::try_from(format!("ws://localhost:{port}/ws"))?;
    let (mut socket, _) = connect_async(url).await?;
    socket.send(Message::Text("over the proxy".into())).await?;
    let echoed = socket
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("the socket closed"))??;
    assert_eq!(echoed.into_text()?.as_str(), "over the proxy");
    Ok(())
}

/// Checks that the first event of an open event stream arrives through the proxy.
async fn check_event_stream(port: u16) -> anyhow::Result<()> {
    let mut events = reqwest::get(format!("http://localhost:{port}/events"))
        .await?
        .error_for_status()?
        .bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("the event stream ended"))??;
    assert!(
        String::from_utf8_lossy(&first).contains("data: ready"),
        "the first event must arrive while the stream stays open"
    );
    Ok(())
}
