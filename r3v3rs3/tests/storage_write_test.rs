use r3v3rs3::config::storage::Storage;
use r3v3rs3::server::rpc::config::{GetConfig, SetConfig};
use r3v3rs3::server::rpc::ports::{DeletePort, GetPortList};
use r3v3rs3::server::rpc::proxies::{DeleteProxy, GetProxyList, UpdateProxy};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::proxy::HttpProxy;
use std::time::Duration;

mod common;
use common::{
    TestStorage, alloc_tcp_port, call, http_port_entry, http_proxy_entry, http_route, with_server,
};

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

#[tokio::test]
async fn a_change_is_not_applied_when_the_storage_cannot_save() -> anyhow::Result<()> {
    let proxy_port = alloc_tcp_port().await?;
    let port = http_port_entry("test", &proxy_port);
    let manual = http_proxy_entry(
        "manual",
        "test",
        HttpProxy {
            routes: vec![http_route("/", "http://127.0.0.1:1/", None)],
            ..Default::default()
        },
    );
    let storage = TestStorage::builder()
        .ports(vec![port.clone()])
        .proxies(vec![manual.clone()])
        .build();
    let mut renamed = manual.clone();
    renamed.proxy.name = "renamed".into();

    let server_storage = storage.clone();
    let expected = vec![renamed.clone()];
    with_server(storage.clone(), |mut channels| async move {
        server_storage
            .set_save_error(Some(Error::FailedToSaveConfig))
            .await;

        let update = UpdateProxy {
            entry: renamed.clone(),
        };
        let updated = call(&mut channels, update).await?;
        assert!(
            matches!(updated, Err(Error::FailedToSaveConfig)),
            "{updated:?}"
        );
        let deleted = call(&mut channels, DeleteProxy { id: manual.id }).await?;
        assert!(
            matches!(deleted, Err(Error::FailedToSaveConfig)),
            "{deleted:?}"
        );
        assert_eq!(call(&mut channels, GetProxyList).await??, [manual]);

        let deleted = call(&mut channels, DeletePort { id: port.id }).await?;
        assert!(
            matches!(deleted, Err(Error::FailedToSaveConfig)),
            "{deleted:?}"
        );
        assert_eq!(call(&mut channels, GetPortList).await??, [port]);

        let config = AppConfig {
            background_task_interval: Duration::from_secs(600),
            ..Default::default()
        };
        let set = call(&mut channels, SetConfig { config }).await?;
        assert!(matches!(set, Err(Error::FailedToSaveConfig)), "{set:?}");
        let current = call(&mut channels, GetConfig).await??;
        assert_eq!(current.background_task_interval, Duration::from_secs(3600));

        server_storage.set_save_error(None).await;
        call(&mut channels, UpdateProxy { entry: renamed }).await??;
        Ok(())
    })
    .await?;

    assert_eq!(storage.load_proxies().await, expected);
    Ok(())
}

#[test]
fn storage_errors_have_their_http_status() {
    assert_eq!(Error::ClusterUnavailable.status_code(), 503);
    assert_eq!(Error::ClusterWriteConflict.status_code(), 409);
    assert_eq!(Error::FailedToSaveConfig.status_code(), 500);
}
