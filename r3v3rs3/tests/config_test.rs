use r3v3rs3::{
    command::ServerCommand,
    config::storage::Storage,
    server::rpc::{config::SetConfig, ErasedRpcMethod, RpcWrapper},
};
use r3v3rs3_api::app::{AcmeExecConfig, AppConfig};
use std::time::Duration;

mod common;
use common::{with_server, TestStorage};

#[tokio::test]
async fn set_config_persists_to_storage() -> anyhow::Result<()> {
    let storage = TestStorage::builder().build();

    let mut config = AppConfig::default();
    config.admin.max_login_attempts = 3;
    config.admin.login_attempts_reset = Duration::from_secs(120);
    config.background_task_interval = Duration::from_secs(600);

    let expected = config.clone();
    with_server(storage.clone(), |mut channels| async move {
        let arg = Box::new(RpcWrapper::new(SetConfig { config })) as Box<dyn ErasedRpcMethod>;
        channels
            .command
            .send(ServerCommand::CallMethod { id: 1, arg })
            .await?;
        let callback = channels
            .callback
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
        assert_eq!(callback.id, 1);
        assert!(callback.result.is_ok());
        Ok(())
    })
    .await?;

    assert_eq!(storage.load_app_config().await, expected);
    Ok(())
}

#[tokio::test]
async fn set_config_keeps_the_exec_programs_of_the_file() -> anyhow::Result<()> {
    let file_config = AppConfig {
        acme_exec: AcmeExecConfig {
            programs: vec!["/usr/local/bin/r3v3rs3-dns-hook".into()],
            timeout: Duration::from_secs(5),
        },
        ..Default::default()
    };
    let storage = TestStorage::builder().config(file_config.clone()).build();

    // An update through the admin API cannot add a program.
    let mut config = AppConfig::default();
    config.acme_exec.programs = vec!["/bin/sh".into()];
    config.background_task_interval = Duration::from_secs(600);

    with_server(storage.clone(), |mut channels| async move {
        let arg = Box::new(RpcWrapper::new(SetConfig { config })) as Box<dyn ErasedRpcMethod>;
        channels
            .command
            .send(ServerCommand::CallMethod { id: 1, arg })
            .await?;
        let callback = channels
            .callback
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
        assert!(callback.result.is_ok());
        Ok(())
    })
    .await?;

    let saved = storage.load_app_config().await;
    assert_eq!(saved.acme_exec, file_config.acme_exec);
    assert_eq!(saved.background_task_interval, Duration::from_secs(600));
    Ok(())
}
