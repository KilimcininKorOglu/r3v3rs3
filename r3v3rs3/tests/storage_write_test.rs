use r3v3rs3::config::storage::Storage;
use r3v3rs3::server::ServerChannels;
use r3v3rs3::server::rpc::config::{GetConfig, SetConfig};
use r3v3rs3::server::rpc::ports::{DeletePort, GetPortList};
use r3v3rs3::server::rpc::proxies::{DeleteProxy, GetProxyList, UpdateProxy};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::proxy::{HttpProxy, ProxyEntry};
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
        .set_write_error(Some(Error::FailedToSaveConfig))
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
            matches!(rejected, Err(Error::FailedToSaveConfig)),
            "{rejected:?}"
        );
        // A read still works, and the rejected change is not applied.
        let current = call(&mut channels, GetConfig).await??;
        assert_eq!(current.background_task_interval, Duration::from_secs(3600));

        // The check runs before the method, so an unknown id is not reported.
        let id = "abcdef".parse()?;
        let deleted = call(&mut channels, DeleteProxy { id }).await?;
        assert!(
            matches!(deleted, Err(Error::FailedToSaveConfig)),
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

        check_proxy_changes_not_applied(&mut channels, &manual, &renamed).await?;
        check_port_delete_not_applied(&mut channels, &port).await?;
        check_config_change_not_applied(&mut channels).await?;

        server_storage.set_save_error(None).await;
        call(&mut channels, UpdateProxy { entry: renamed }).await??;
        Ok(())
    })
    .await?;

    assert_eq!(storage.load_proxies().await, expected);
    Ok(())
}

/// Checks that a failed save rejects the update and the delete of a proxy and keeps the old
/// proxy.
async fn check_proxy_changes_not_applied(
    channels: &mut ServerChannels,
    manual: &ProxyEntry,
    renamed: &ProxyEntry,
) -> anyhow::Result<()> {
    let update = UpdateProxy {
        entry: renamed.clone(),
    };
    let updated = call(channels, update).await?;
    assert!(
        matches!(updated, Err(Error::FailedToSaveConfig)),
        "{updated:?}"
    );
    let deleted = call(channels, DeleteProxy { id: manual.id }).await?;
    assert!(
        matches!(deleted, Err(Error::FailedToSaveConfig)),
        "{deleted:?}"
    );
    let proxies = call(channels, GetProxyList).await??;
    assert_eq!(proxies.len(), 1);
    assert_eq!(proxies[0].id, manual.id);
    assert_eq!(proxies[0].proxy, manual.proxy);
    Ok(())
}

/// Checks that a failed save rejects the delete of a port and keeps the port.
async fn check_port_delete_not_applied(
    channels: &mut ServerChannels,
    port: &PortEntry,
) -> anyhow::Result<()> {
    let deleted = call(channels, DeletePort { id: port.id }).await?;
    assert!(
        matches!(deleted, Err(Error::FailedToSaveConfig)),
        "{deleted:?}"
    );
    let ports = call(channels, GetPortList).await??;
    assert_eq!(ports.len(), 1);
    assert_eq!(ports[0].id, port.id);
    assert_eq!(ports[0].port, port.port);
    Ok(())
}

/// Checks that a failed save rejects a config change and keeps the old config.
async fn check_config_change_not_applied(channels: &mut ServerChannels) -> anyhow::Result<()> {
    let config = AppConfig {
        background_task_interval: Duration::from_secs(600),
        ..Default::default()
    };
    let set = call(channels, SetConfig { config }).await?;
    assert!(matches!(set, Err(Error::FailedToSaveConfig)), "{set:?}");
    let current = call(channels, GetConfig).await??;
    assert_eq!(current.background_task_interval, Duration::from_secs(3600));
    Ok(())
}

#[test]
fn storage_errors_have_their_http_status() {
    assert_eq!(Error::FailedToSaveConfig.status_code(), 500);
}

#[tokio::test]
async fn list_requests_return_proxy_and_port_statuses() -> anyhow::Result<()> {
    let proxy_port = alloc_tcp_port().await?;
    let port = http_port_entry("port1", &proxy_port);
    let manual = http_proxy_entry(
        "proxy1",
        "port1",
        HttpProxy {
            routes: vec![http_route("/", "http://127.0.0.1:1/", None)],
            ..Default::default()
        },
    );
    let storage = TestStorage::builder()
        .ports(vec![port.clone()])
        .proxies(vec![manual.clone()])
        .build();

    with_server(storage, |mut channels| async move {
        let proxies = call(&mut channels, GetProxyList).await??;
        assert_eq!(proxies.len(), 1);
        assert!(proxies[0].status.is_some());
        assert_eq!(
            proxies[0].status.as_ref().unwrap().state,
            r3v3rs3_api::proxy::ProxyState::Active
        );

        let ports = call(&mut channels, GetPortList).await??;
        assert_eq!(ports.len(), 1);
        assert!(ports[0].status.is_some());
        assert_eq!(
            ports[0].status.as_ref().unwrap().state.socket,
            r3v3rs3_api::port::SocketState::Listening
        );
        Ok(())
    })
    .await?;
    Ok(())
}
