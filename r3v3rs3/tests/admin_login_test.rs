use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::{
    app::AppConfig,
    auth::{LoginMethod, LoginRequest},
};
use std::{collections::HashMap, net::SocketAddr, time::Duration};
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{alloc_tcp_port, wait_for_listener, with_server, TestStorage};

// The admin login route also has a governor layer that allows a burst of 2
// and replenishes one request every 4 seconds.
const GOVERNOR_REPLENISH: Duration = Duration::from_millis(4500);

async fn login(addr: SocketAddr, username: &str, password: &str) -> anyhow::Result<u16> {
    let res = reqwest::Client::new()
        .post(format!("http://{addr}/api/login"))
        .json(&LoginRequest {
            username: username.to_string(),
            method: LoginMethod::Password {
                password: password.to_string(),
            },
            insecure: true,
        })
        .send()
        .await?;
    Ok(res.status().as_u16())
}

#[tokio::test]
async fn login_is_blocked_after_max_attempts() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-admin-login-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;

    let admin_port = alloc_tcp_port().await?;
    let addr = admin_port.socket_addr();

    let mut config = AppConfig::default();
    config.admin.max_login_attempts = 2;
    config.admin.login_attempts_reset = Duration::from_secs(60);

    let storage = TestStorage::builder()
        .config(config)
        .accounts(HashMap::from([
            ("admin".to_string(), "secret".to_string()),
            ("other".to_string(), "secret".to_string()),
        ]))
        .build();

    let app_info = new_appinfo(&dir, &dir);
    with_server(storage, |channels| async move {
        tokio::spawn(start_admin(
            app_info,
            addr,
            channels.command,
            channels.callback,
            channels.event.clone(),
            channels.accounts.clone(),
        ));
        wait_for_listener(addr).await?;

        assert_eq!(login(addr, "admin", "wrong").await?, 400);
        assert_eq!(login(addr, "admin", "wrong").await?, 400);

        // The correct password is rejected while the client IP and username are blocked.
        tokio::time::sleep(GOVERNOR_REPLENISH).await;
        assert_eq!(login(addr, "admin", "secret").await?, 429);

        // Another username from the same client IP is not blocked.
        tokio::time::sleep(GOVERNOR_REPLENISH).await;
        assert_eq!(login(addr, "other", "secret").await?, 200);
        Ok(())
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
