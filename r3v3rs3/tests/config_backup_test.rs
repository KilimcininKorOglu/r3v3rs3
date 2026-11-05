use base64::Engine as _;
use r3v3rs3::{
    accounts::Caller,
    certs::acme::AcmeEntry,
    command::ServerCommand,
    config::storage::Storage,
    server::rpc::{
        ErasedRpcMethod, RpcWrapper,
        config::{ExportConfigBackup, ImportConfigBackup},
    },
};
use r3v3rs3_api::{
    access_list::{AccessList, AccessListEntry},
    acme::{Acme, AcmeConfig, HTTP_01},
    app::AppConfig,
    config_backup::ConfigBackup,
    policy::{AuthPolicy, BasicAuth, BasicAuthUser},
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
            ip_filter: Default::default(),
            auth: AuthPolicy::Basic(BasicAuth {
                realm: "Test".into(),
                user_file: None,
                users: vec![BasicAuthUser {
                    username: "alice".into(),
                    password: "".into(),
                    password_hash: "$argon2id$mockedhash".into(),
                    password_set: true,
                }],
            }),
        },
    };
    storage
        .save_access_lists(&[access_list_entry.clone()])
        .await?;

    let key = rcgen::KeyPair::generate()?;
    let account = serde_json::from_value(serde_json::json!({
        "id": "https://acme.example/acct/1",
        "key_pkcs8": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.serialize_der()),
        "directory": "https://acme.example/directory",
    }))?;
    let acme_entry = AcmeEntry {
        id: "acme-1".parse()?,
        acme: Acme {
            config: AcmeConfig::default(),
            identifiers: vec!["example.com".parse()?],
            challenge_type: HTTP_01.to_string(),
            dns_provider: None,
            dns_credential: None,
            dns_alias: None,
        },
        account: Arc::new(account),
    };
    storage.save_acme(&acme_entry).await?;

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
        assert_eq!(backup.acme.len(), 1);
        assert_eq!(backup.config.admin.max_login_attempts, 5);

        if let AuthPolicy::Basic(basic) = &backup.access_lists[0].list.auth {
            assert!(basic.users[0].password_hash.is_empty());
            assert!(basic.users[0].password_set);
        }

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

    // Target storage with existing access list to verify secrets preservation
    let target_storage = TestStorage::builder().build();
    target_storage
        .save_access_lists(&[access_list_entry])
        .await?;

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
    assert_eq!(target_storage.load_acmes().await.len(), 1);
    let restored_lists = target_storage.load_access_lists().await;
    if let AuthPolicy::Basic(basic) = &restored_lists[0].list.auth {
        assert_eq!(basic.users[0].password_hash, "$argon2id$mockedhash");
    }
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
