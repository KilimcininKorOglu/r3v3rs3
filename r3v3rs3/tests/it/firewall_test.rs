use r3v3rs3::{
    admin::start_admin, config::new_appinfo, log::DatabaseLayer, server::ServerChannels,
};
use r3v3rs3_api::app::{AppConfig, AppInfo};
use r3v3rs3_api::auth::Role;
use reqwest::Method;
use serde_json::{Value, json};
use std::net::SocketAddr;
use tracing_subscriber::filter::LevelFilter;

use crate::common;
use common::{TestStorage, alloc_tcp_port, send, session_cookie, wait_for_listener, with_server};

/// The host firewall is off until an admin turns it on, any account can read its state, and
/// only an admin can change it. A host port outside 1-65535 or of another protocol is refused,
/// so a typo never reaches the nft script.
#[tokio::test]
async fn only_an_admin_changes_the_firewall_and_bad_ports_are_refused() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-firewall-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;
    let addr = alloc_tcp_port().await?.socket_addr();
    let storage = TestStorage::builder()
        .account("admin", "admin-secret", Role::Admin, None)
        .account("viewer", "viewer-secret", Role::Viewer, None)
        .build();
    let app_info = new_appinfo(&dir, &dir);
    let result = with_server(storage, |channels| check_firewall(channels, app_info, addr)).await;
    std::fs::remove_dir_all(&dir)?;
    result
}

async fn check_firewall(
    channels: ServerChannels,
    app_info: AppInfo,
    addr: SocketAddr,
) -> anyhow::Result<()> {
    tokio::spawn(start_admin(
        app_info,
        addr,
        channels.command,
        channels.callback,
        channels.event.clone(),
        channels.accounts.clone(),
    ));
    wait_for_listener(addr).await?;
    let admin = session_cookie(addr, "admin", "admin-secret").await?;
    let viewer = session_cookie(addr, "viewer", "viewer-secret").await?;
    let config = check_viewer(addr, &viewer).await?;
    check_bad_ports(addr, &admin, &config).await?;
    check_saved(addr, &admin, config).await
}

/// A viewer reads the state of the firewall, which starts off, but cannot change it. Returns
/// the config with one extra port.
async fn check_viewer(addr: SocketAddr, viewer: &str) -> anyhow::Result<Value> {
    let (status, body) = send(addr, Method::GET, "/api/firewall", viewer, None).await?;
    assert_eq!(status, 200, "{body}");
    let state: Value = serde_json::from_str(&body)?;
    assert_eq!(state["enabled"], false, "{body}");

    let mut config = serde_json::to_value(AppConfig::default())?;
    assert_eq!(
        config["firewall"]["enabled"], false,
        "the firewall must start off"
    );
    config["firewall"] = json!({"enabled": false, "extra_ports": ["udp/51820"]});
    let (status, _) = send(
        addr,
        Method::PUT,
        "/api/config",
        viewer,
        Some(config.clone()),
    )
    .await?;
    assert_eq!(status, 403);
    Ok(config)
}

async fn check_bad_ports(addr: SocketAddr, admin: &str, config: &Value) -> anyhow::Result<()> {
    for bad in ["udp/0", "udp/70000", "icmp/1", "tcp/9-1", "udp/51820 "] {
        let mut hostile = config.clone();
        hostile["firewall"]["extra_ports"] = json!([bad]);
        let (status, body) = send(addr, Method::PUT, "/api/config", admin, Some(hostile)).await?;
        assert!(
            (400..500).contains(&status),
            "{bad:?} gave {status}: {body}"
        );
    }
    Ok(())
}

async fn check_saved(addr: SocketAddr, admin: &str, config: Value) -> anyhow::Result<()> {
    let (status, body) = send(addr, Method::PUT, "/api/config", admin, Some(config)).await?;
    assert_eq!(status, 200, "{body}");
    let (_, body) = send(addr, Method::GET, "/api/config", admin, None).await?;
    let saved: Value = serde_json::from_str(&body)?;
    assert_eq!(
        saved["firewall"]["extra_ports"],
        json!(["udp/51820"]),
        "{body}"
    );
    Ok(())
}
