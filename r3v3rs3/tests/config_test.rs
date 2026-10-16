use r3v3rs3::{
    accounts::Caller,
    command::ServerCommand,
    config::storage::Storage,
    server::rpc::{ErasedRpcMethod, RpcWrapper, config::SetConfig},
};
use r3v3rs3_api::app::{AcmeExecConfig, AppConfig, Http2Limits, Http3Limits};
use std::time::Duration;

mod common;
use common::{TestStorage, with_server};

#[tokio::test]
async fn set_config_persists_to_storage() -> anyhow::Result<()> {
    let storage = TestStorage::builder().build();

    let mut config = AppConfig::default();
    config.admin.max_login_attempts = 3;
    config.admin.login_attempts_reset = Duration::from_secs(120);
    config.background_task_interval = Duration::from_secs(600);
    config.upstream_dns_resolver = Some("127.0.0.1:8600".parse()?);
    config.http2 = Http2Limits {
        max_header_list_size: 128 * 1024,
        max_pending_accept_reset_streams: 256,
    };
    config.http3 = Http3Limits {
        max_concurrent_bidi_streams: 200,
        max_concurrent_uni_streams: 20,
    };

    let expected = config.clone();
    with_server(storage.clone(), |mut channels| async move {
        let arg = Box::new(RpcWrapper::new(SetConfig { config })) as Box<dyn ErasedRpcMethod>;
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
        assert!(callback.result.is_ok());
        Ok(())
    })
    .await?;

    assert_eq!(storage.load_app_config().await, expected);
    Ok(())
}

#[tokio::test]
async fn set_config_carries_the_exec_programs_of_the_file() -> anyhow::Result<()> {
    let file_config = AppConfig {
        acme_exec: AcmeExecConfig {
            programs: vec!["/usr/local/bin/r3v3rs3-dns-hook".into()],
            timeout: Duration::from_secs(5),
        },
        ..Default::default()
    };
    let storage = TestStorage::builder().config(file_config.clone()).build();

    // The exec allowlist travels with the settings.
    let mut config = AppConfig::default();
    config.acme_exec.programs = vec!["/bin/sh".into()];
    config.acme_exec.timeout = Duration::from_secs(9);
    config.background_task_interval = Duration::from_secs(600);

    with_server(storage.clone(), |mut channels| async move {
        let arg = Box::new(RpcWrapper::new(SetConfig { config })) as Box<dyn ErasedRpcMethod>;
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
        assert!(callback.result.is_ok());
        Ok(())
    })
    .await?;

    let saved = storage.load_app_config().await;
    assert_eq!(
        saved.acme_exec,
        AcmeExecConfig {
            programs: vec!["/bin/sh".into()],
            timeout: Duration::from_secs(9),
        }
    );
    assert_eq!(saved.background_task_interval, Duration::from_secs(600));
    Ok(())
}
