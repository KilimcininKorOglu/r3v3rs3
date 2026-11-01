use r3v3rs3::{
    accounts::Caller,
    command::ServerCommand,
    config::storage::Storage,
    server::rpc::{
        ErasedRpcMethod, RpcWrapper,
        config::{ExportConfigBackup, ImportConfigBackup},
    },
};
use r3v3rs3_api::{
    access_list::{AccessList, AccessListEntry},
    app::AppConfig,
    config_backup::ConfigBackup,
    proxy::HttpProxy,
};
use std::sync::{Arc, Mutex};

mod common;
use common::{TestStorage, http_proxy_entry, port_entry, with_server};

#[tokio::test]
async fn export_and_import_config_backup_round_trip() -> anyhow::Result<()> {
    let storage = TestStorage::builder().build();

    let p_entry = port_entry("port-1", "/ip4/127.0.0.1/tcp/9090".parse()?);
    storage.save_ports(&[p_entry]).await?;

    let pr_entry = http_proxy_entry("proxy-1", "port-1", HttpProxy::default());
    storage.save_proxies(&[pr_entry]).await?;

    let access_list_entry = AccessListEntry {
        id: "list-1".parse()?,
        list: AccessList {
            name: "trusted".into(),
            ..Default::default()
        },
    };
    storage.save_access_lists(&[access_list_entry]).await?;

    let mut config = AppConfig::default();
    config.admin.max_login_attempts = 5;
    storage.save_app_config(&config).await?;

    let exported_backup = Arc::new(Mutex::new(None));
    let exported_clone = exported_backup.clone();

    with_server(storage.clone(), |mut channels| async move {
        let arg = Box::new(RpcWrapper::new(ExportConfigBackup)) as Box<dyn ErasedRpcMethod>;
        channels
            .command
            .send(ServerCommand::CallMethod {
                id: 1,
                arg,
                caller: Caller::system(),
            })
            .await?;
        let callback = channels
            .callback
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
        assert_eq!(callback.id, 1);
        let backup = callback
            .result?
            .downcast::<ConfigBackup>()
            .map_err(|_| anyhow::anyhow!("downcast failed"))?;

        assert_eq!(backup.ports.len(), 1);
        assert_eq!(backup.proxies.len(), 1);
        assert_eq!(backup.access_lists.len(), 1);
        assert_eq!(backup.config.admin.max_login_attempts, 5);

        assert!(backup.files.contains_key("config.toml"));
        assert!(backup.files.contains_key("ports.toml"));
        assert!(backup.files.contains_key("proxies.toml"));
        assert!(backup.files.contains_key("access_lists.toml"));
        assert!(!backup.files.contains_key("accounts.toml"));

        *exported_clone.lock().unwrap() = Some(*backup);
        Ok(())
    })
    .await?;

    let backup = exported_backup
        .lock()
        .unwrap()
        .take()
        .expect("exported backup missing");

    // Clear storage to simulate a fresh second server
    let target_storage = TestStorage::builder().build();

    with_server(target_storage.clone(), |mut channels| async move {
        let arg = Box::new(RpcWrapper::new(ImportConfigBackup {
            backup: Box::new(backup),
        })) as Box<dyn ErasedRpcMethod>;
        channels
            .command
            .send(ServerCommand::CallMethod {
                id: 2,
                arg,
                caller: Caller::system(),
            })
            .await?;
        let callback = channels
            .callback
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
        assert_eq!(callback.id, 2);
        assert!(callback.result.is_ok());
        Ok(())
    })
    .await?;

    assert_eq!(target_storage.load_ports().await.len(), 1);
    assert_eq!(target_storage.load_proxies().await.len(), 1);
    assert_eq!(target_storage.load_access_lists().await.len(), 1);
    assert_eq!(
        target_storage
            .load_app_config()
            .await
            .admin
            .max_login_attempts,
        5
    );

    Ok(())
}

#[tokio::test]
async fn config_restore_accepts_payload_larger_than_default_limit() -> anyhow::Result<()> {
    use common::{alloc_tcp_port, login_when_allowed, wait_for_listener};
    use r3v3rs3::admin::start_admin;
    use r3v3rs3::config::new_appinfo;
    use r3v3rs3::server::Server;
    use r3v3rs3_api::auth::Role;

    let dir = std::env::temp_dir().join(format!("r3-cfg-restore-{}", rand::random::<u32>()));
    std::fs::create_dir_all(&dir)?;

    let storage = TestStorage::builder()
        .account("admin", "password", Role::Admin, None)
        .build();
    let addr = alloc_tcp_port().await?.socket_addr();
    let (server, channels) = Server::new(new_appinfo(&dir, &dir), storage).await;
    let event = channels.event.clone();
    let task = tokio::spawn(server.start());
    tokio::spawn(start_admin(
        new_appinfo(&dir, &dir),
        addr,
        channels.command,
        channels.callback,
        channels.event.clone(),
        channels.accounts.clone(),
    ));
    wait_for_listener(addr).await?;

    let cookie = login_when_allowed(addr, "admin", "password").await?;
    let mut files = std::collections::BTreeMap::new();
    let large_comment = "# ".to_string() + &"x".repeat(2_500_000) + "\n";
    files.insert("proxies.toml".to_string(), large_comment);

    let backup = ConfigBackup {
        version: "3.0.1".to_string(),
        created_at: 0,
        config: AppConfig::default(),
        ports: Vec::new(),
        proxies: Vec::new(),
        access_lists: Vec::new(),
        acme: Vec::new(),
        files,
    };

    let client = reqwest::Client::new();
    let res = client
        .post(format!("http://{addr}/api/config/restore"))
        .header("cookie", &cookie)
        .json(&backup)
        .send()
        .await?;

    assert_ne!(res.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    event.send(r3v3rs3_api::event::ServerEvent::Shutdown)?;
    task.await??;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
