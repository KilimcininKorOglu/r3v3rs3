use mockito::Matcher;
use r3v3rs3::server::{
    ServerChannels,
    rpc::{
        access_lists::{DeleteAccessList, UpdateAccessList},
        proxies::UpdateProxy,
    },
};
use r3v3rs3_api::{
    access_list::{AccessList, AccessListEntry},
    error::Error,
    id::ShortId,
    policy::IpFilter,
    proxy::{HttpProxy, ProxyEntry, ProxyKind, Route},
};

mod common;
use common::{
    TestPort, TestStorage, alloc_tcp_port, call, http_port_entry, http_proxy_entry, http_route,
    wait_for_status, with_server,
};

fn office(deny: &[&str]) -> AccessListEntry {
    let list = AccessList {
        name: "Office".into(),
        ip_filter: IpFilter {
            deny: deny.iter().map(|net| net.parse().unwrap()).collect(),
            ..Default::default()
        },
        ..Default::default()
    };
    ("office".parse::<ShortId>().unwrap(), list).into()
}

fn listed_route(path: &str, upstream: &str, list: &str) -> Route {
    Route {
        access_list: Some(list.parse().unwrap()),
        ..http_route(path, upstream, None)
    }
}

fn proxy(routes: Vec<Route>) -> HttpProxy {
    HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes,
        ..Default::default()
    }
}

#[tokio::test]
async fn proxies_and_routes_use_the_current_access_lists() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let upstream = server.url();
    let _mock = server
        .mock("GET", Matcher::Any)
        .with_body("ok")
        .create_async()
        .await;

    let loopback = ["127.0.0.0/8", "::1/128"];
    let routes = vec![
        listed_route("/office", &upstream, "office"),
        listed_route("/gone", &upstream, "gone"),
        http_route("/", &upstream, None),
    ];
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("proxy", "http", proxy(routes))])
        .access_lists(vec![office(&loopback)])
        .build();

    with_server(config, |mut channels| async move {
        check_lists_apply_live(&mut channels, &port).await?;
        let id = check_used_list_stays(&mut channels).await?;
        check_missing_list_is_rejected(&mut channels, &upstream).await?;
        let entry = check_conflict_is_rejected(&mut channels, &upstream, id, &loopback).await?;
        check_rejected_updates_keep_the_proxy(&mut channels, &port, entry).await
    })
    .await
}

/// Returns the status of a GET request to the URL.
async fn status_of(url: url::Url) -> anyhow::Result<reqwest::StatusCode> {
    Ok(reqwest::get(url).await?.status())
}

/// Checks that the route lists deny the clients and that a changed list applies without a restart.
async fn check_lists_apply_live(
    channels: &mut ServerChannels,
    port: &TestPort,
) -> anyhow::Result<()> {
    wait_for_status(port.http_url("/").as_str(), 200).await?;
    // The list of the route denies the loopback clients, and a missing list denies every client.
    assert_eq!(status_of(port.http_url("/office")).await?, 403);
    assert_eq!(status_of(port.http_url("/gone")).await?, 403);

    // The proxy uses the changed list without a restart.
    call(channels, UpdateAccessList { entry: office(&[]) }).await??;
    wait_for_status(port.http_url("/office").as_str(), 200).await?;
    Ok(())
}

/// Checks that a list in use cannot be deleted, and returns its id.
async fn check_used_list_stays(channels: &mut ServerChannels) -> anyhow::Result<ShortId> {
    let id = "office".parse()?;
    let result = call(channels, DeleteAccessList { id }).await?;
    assert!(
        matches!(result, Err(Error::AccessListInUse { id: used }) if used == id),
        "{result:?}"
    );
    Ok(id)
}

/// Checks that a proxy update with a missing list is rejected.
async fn check_missing_list_is_rejected(
    channels: &mut ServerChannels,
    upstream: &str,
) -> anyhow::Result<()> {
    let missing = HttpProxy {
        access_list: Some("nope".parse()?),
        ..proxy(vec![http_route("/", upstream, None)])
    };
    let entry = http_proxy_entry("proxy", "http", missing);
    let result = call(channels, UpdateProxy { entry }).await?;
    let nope: ShortId = "nope".parse()?;
    assert!(
        matches!(result, Err(Error::AccessListNotFound { id: missing }) if missing == nope),
        "{result:?}"
    );
    Ok(())
}

/// Checks that a proxy with both a list and its own IP filter is rejected, and returns the entry.
async fn check_conflict_is_rejected(
    channels: &mut ServerChannels,
    upstream: &str,
    id: ShortId,
    loopback: &[&str],
) -> anyhow::Result<ProxyEntry> {
    let conflict = HttpProxy {
        access_list: Some(id),
        ip_filter: office(loopback).list.ip_filter,
        ..proxy(vec![http_route("/", upstream, None)])
    };
    let entry = http_proxy_entry("proxy", "http", conflict);
    let result = call(
        channels,
        UpdateProxy {
            entry: entry.clone(),
        },
    )
    .await?;
    assert!(
        matches!(result, Err(Error::AccessListConflict)),
        "{result:?}"
    );
    Ok(entry)
}

/// Checks that the rejected updates keep the proxy, and that the fixed entry applies.
async fn check_rejected_updates_keep_the_proxy(
    channels: &mut ServerChannels,
    port: &TestPort,
    mut entry: ProxyEntry,
) -> anyhow::Result<()> {
    // The rejected updates keep the proxy.
    assert_eq!(status_of(port.http_url("/gone")).await?, 403);
    if let ProxyKind::Http(http) = &mut entry.proxy.kind {
        http.ip_filter = IpFilter::default();
    }
    call(channels, UpdateProxy { entry }).await??;
    wait_for_status(port.http_url("/gone").as_str(), 200).await?;
    Ok(())
}
