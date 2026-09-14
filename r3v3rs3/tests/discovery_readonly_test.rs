use axum::{routing::get, Router};
use r3v3rs3::{
    command::ServerCommand,
    config::storage::Storage,
    discovery::{DiscoveredProxy, DiscoverySnapshot, ProxyDefinition},
    server::rpc::{
        discovery::GetDiscoveryStatus,
        ports::UpdatePort,
        proxies::{DeleteProxy, GetProxyList, UpdateProxy},
    },
    server::ServerChannels,
};
use r3v3rs3_api::{
    discovery::{DiscoveryIssue, DiscoveryProvider, DiscoverySource, DiscoveryState},
    error::Error,
    port::PortEntry,
    proxy::{HttpProxy, ProxyEntry, ProxyKind},
};

mod common;
use common::{
    alloc_tcp_port, call, http_port_entry, http_proxy_entry, http_route, serve_http_upstream,
    wait_for_status, with_server, TestStorage,
};

fn discovered(name: &str, port: &str, upstream: &str) -> DiscoveredProxy {
    let http = HttpProxy {
        routes: vec![http_route("/", upstream, None)],
        ..Default::default()
    };
    DiscoveredProxy {
        key: format!("{name}/http.app"),
        source: DiscoverySource {
            provider: DiscoveryProvider::Docker,
            resource: name.into(),
        },
        definition: ProxyDefinition {
            key: "http.app".into(),
            name: name.into(),
            ports: vec![port.into()],
            active: true,
            kind: ProxyKind::Http(Box::new(http)),
        },
    }
}

async fn send_snapshot(
    channels: &mut ServerChannels,
    state: DiscoveryState,
    proxies: Option<Vec<DiscoveredProxy>>,
) -> anyhow::Result<()> {
    let snapshot = DiscoverySnapshot {
        provider: DiscoveryProvider::Docker,
        generation: 0,
        state,
        error: None,
        proxies,
        certs: vec![],
        issues: vec![],
    };
    channels
        .command
        .send(ServerCommand::SetDiscovery { snapshot })
        .await?;
    Ok(())
}

async fn discovered_entry(channels: &mut ServerChannels) -> anyhow::Result<ProxyEntry> {
    let entries = call(channels, GetProxyList).await??;
    let mut discovered = entries.into_iter().filter(ProxyEntry::is_discovered);
    let entry = discovered
        .next()
        .ok_or_else(|| anyhow::anyhow!("no discovered proxy"))?;
    anyhow::ensure!(
        discovered.next().is_none(),
        "more than one discovered proxy"
    );
    Ok(entry)
}

struct Setup {
    upstream: String,
    url: String,
    port: PortEntry,
    manual: ProxyEntry,
    storage: TestStorage,
}

/// An upstream server, a port with the name `web` and a manual proxy on the port.
async fn setup() -> anyhow::Result<Setup> {
    let upstream =
        serve_http_upstream(Router::new().route("/", get(|| async { "discovered" }))).await?;
    let proxy_port = alloc_tcp_port().await?;
    let manual = http_proxy_entry(
        "manual",
        "test",
        HttpProxy {
            vhosts: vec!["manual.example".parse()?],
            routes: vec![http_route("/", upstream.as_str(), None)],
            ..Default::default()
        },
    );
    let mut port = http_port_entry("test", &proxy_port);
    port.port.name = "web".into();
    let storage = TestStorage::builder()
        .ports(vec![port.clone()])
        .proxies(vec![manual.clone()])
        .build();
    Ok(Setup {
        upstream: upstream.to_string(),
        url: proxy_port.http_url("/").to_string(),
        port,
        manual,
        storage,
    })
}

#[tokio::test]
async fn discovered_proxies_serve_traffic_but_are_read_only_and_not_saved() -> anyhow::Result<()> {
    let Setup {
        upstream,
        url,
        manual,
        storage,
        ..
    } = setup().await?;
    let mut renamed = manual.clone();
    renamed.proxy.name = "renamed".into();
    let saved = vec![renamed.clone()];
    with_server(storage.clone(), |mut channels| async move {
        let proxies = vec![discovered("web-1", "web", &upstream)];
        send_snapshot(&mut channels, DiscoveryState::Running, Some(proxies)).await?;
        assert_eq!(wait_for_status(&url, 200).await?, "discovered");

        let entry = discovered_entry(&mut channels).await?;
        let update = UpdateProxy {
            entry: ProxyEntry::from((entry.id, entry.proxy.clone())),
        };
        assert!(matches!(
            call(&mut channels, update).await?,
            Err(Error::ProxyReadOnly { .. })
        ));
        let delete = DeleteProxy { id: entry.id };
        assert!(matches!(
            call(&mut channels, delete).await?,
            Err(Error::ProxyReadOnly { .. })
        ));

        // Changing the manual proxy writes the proxy list to the storage.
        let update = UpdateProxy {
            entry: ProxyEntry::from((renamed.id, renamed.proxy.clone())),
        };
        call(&mut channels, update).await??;

        send_snapshot(&mut channels, DiscoveryState::Running, Some(vec![])).await?;
        // A request that matches no route receives 502 Bad Gateway.
        wait_for_status(&url, 502).await?;
        Ok(())
    })
    .await?;

    assert_eq!(storage.load_proxies().await, saved);
    Ok(())
}

#[tokio::test]
async fn discovery_status_reports_issues_and_follows_port_names() -> anyhow::Result<()> {
    let Setup {
        upstream,
        url,
        port,
        storage,
        ..
    } = setup().await?;
    with_server(storage, |mut channels| async move {
        let proxies = vec![
            discovered("web-1", "web", &upstream),
            discovered("web-2", "missing", &upstream),
        ];
        send_snapshot(&mut channels, DiscoveryState::Running, Some(proxies)).await?;
        assert_eq!(wait_for_status(&url, 200).await?, "discovered");
        let entry = discovered_entry(&mut channels).await?;
        assert_eq!(entry.proxy.ports, [port.id]);

        let statuses = call(&mut channels, GetDiscoveryStatus).await??;
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].state, DiscoveryState::Running);
        assert_eq!(statuses[0].proxies, 1);
        assert_eq!(
            statuses[0].issues,
            [DiscoveryIssue {
                resource: "web-2".into(),
                message: "http.app: port not found: missing".into(),
            }]
        );

        // A snapshot without proxies keeps the proxies of the previous snapshot.
        send_snapshot(&mut channels, DiscoveryState::Error, None).await?;
        let statuses = call(&mut channels, GetDiscoveryStatus).await??;
        assert_eq!(statuses[0].state, DiscoveryState::Error);
        assert_eq!(wait_for_status(&url, 200).await?, "discovered");

        // The definition names the port `web`, so renaming the port removes the proxy.
        let mut renamed_port = port.clone();
        renamed_port.port.name = "other".into();
        call(
            &mut channels,
            UpdatePort {
                entry: renamed_port,
            },
        )
        .await??;
        wait_for_status(&url, 502).await?;
        let statuses = call(&mut channels, GetDiscoveryStatus).await??;
        assert_eq!(statuses[0].proxies, 0);
        assert_eq!(statuses[0].issues.len(), 2);

        // The proxy comes back with the same id.
        call(&mut channels, UpdatePort { entry: port }).await??;
        assert_eq!(wait_for_status(&url, 200).await?, "discovered");
        assert_eq!(discovered_entry(&mut channels).await?.id, entry.id);
        Ok(())
    })
    .await
}
