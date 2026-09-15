use r3v3rs3::config::storage::Storage;
use r3v3rs3::server::rpc::config::{GetConfig, SetConfig};
use r3v3rs3::server::rpc::proxies::DeleteProxy;
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::error::Error;
use std::time::Duration;

mod common;
use common::{call, with_server, TestStorage};

#[tokio::test]
async fn a_change_is_rejected_and_not_applied_when_the_storage_cannot_write() -> anyhow::Result<()>
{
    let storage = TestStorage::builder().build();
    storage
        .set_write_error(Some(Error::ClusterUnavailable))
        .await;

    let config = AppConfig {
        background_task_interval: Duration::from_secs(600),
        ..Default::default()
    };
    let expected = config.clone();
    let server_storage = storage.clone();
    with_server(storage.clone(), |mut channels| async move {
        let rejected = call(
            &mut channels,
            SetConfig {
                config: config.clone(),
            },
        )
        .await?;
        assert!(
            matches!(rejected, Err(Error::ClusterUnavailable)),
            "{rejected:?}"
        );
        // A read still works, and the rejected change is not applied.
        let current = call(&mut channels, GetConfig).await??;
        assert_eq!(current.background_task_interval, Duration::from_secs(3600));

        // The check runs before the method, so an unknown id is not reported.
        let id = "abcdef".parse()?;
        let deleted = call(&mut channels, DeleteProxy { id }).await?;
        assert!(
            matches!(deleted, Err(Error::ClusterUnavailable)),
            "{deleted:?}"
        );

        server_storage.set_write_error(None).await;
        call(&mut channels, SetConfig { config }).await??;
        Ok(())
    })
    .await?;

    assert_eq!(storage.load_app_config().await, expected);
    Ok(())
}

#[test]
fn cluster_errors_have_their_http_status() {
    assert_eq!(Error::ClusterUnavailable.status_code(), 503);
    assert_eq!(Error::ClusterWriteConflict.status_code(), 409);
}
