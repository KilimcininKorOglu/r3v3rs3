use mockito::Matcher;
use r3v3rs3::server::rpc::{
    access_lists::{DeleteAccessList, UpdateAccessList},
    proxies::UpdateProxy,
};
use r3v3rs3_api::{
    access_list::{AccessList, AccessListEntry},
    error::Error,
    id::ShortId,
    policy::IpFilter,
    proxy::{HttpProxy, ProxyKind, Route},
};

mod common;
use common::{
    TestStorage, alloc_tcp_port, call, http_port_entry, http_proxy_entry, http_route,
    wait_for_status, with_server,
};

fn office(deny: &[&str]) -> AccessListEntry {
    let list = AccessList {
        name: "Office".into(),
        ip_filter: IpFilter {
            allow: Vec::new(),
            deny: deny.iter().map(|net| net.parse().unwrap()).collect(),
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
        wait_for_status(port.http_url("/").as_str(), 200).await?;
        // The list of the route denies the loopback clients, and a missing list denies every client.
        assert_eq!(reqwest::get(port.http_url("/office")).await?.status(), 403);
        assert_eq!(reqwest::get(port.http_url("/gone")).await?.status(), 403);

        // The proxy uses the changed list without a restart.
        call(&mut channels, UpdateAccessList { entry: office(&[]) }).await??;
        wait_for_status(port.http_url("/office").as_str(), 200).await?;

        let id = "office".parse()?;
        let result = call(&mut channels, DeleteAccessList { id }).await?;
        assert!(
            matches!(result, Err(Error::AccessListInUse { id: used }) if used == id),
            "{result:?}"
        );

        let missing = HttpProxy {
            access_list: Some("nope".parse()?),
            ..proxy(vec![http_route("/", &upstream, None)])
        };
        let entry = http_proxy_entry("proxy", "http", missing);
        let result = call(&mut channels, UpdateProxy { entry }).await?;
        let nope: ShortId = "nope".parse()?;
        assert!(
            matches!(result, Err(Error::AccessListNotFound { id: missing }) if missing == nope),
            "{result:?}"
        );

        let conflict = HttpProxy {
            access_list: Some(id),
            ip_filter: office(&loopback).list.ip_filter,
            ..proxy(vec![http_route("/", &upstream, None)])
        };
        let mut entry = http_proxy_entry("proxy", "http", conflict);
        let result = call(
            &mut channels,
            UpdateProxy {
                entry: entry.clone(),
            },
        )
        .await?;
        assert!(
            matches!(result, Err(Error::AccessListConflict)),
            "{result:?}"
        );

        // The rejected updates keep the proxy.
        assert_eq!(reqwest::get(port.http_url("/gone")).await?.status(), 403);
        if let ProxyKind::Http(http) = &mut entry.proxy.kind {
            http.ip_filter = IpFilter::default();
        }
        call(&mut channels, UpdateProxy { entry }).await??;
        wait_for_status(port.http_url("/gone").as_str(), 200).await?;
        Ok(())
    })
    .await
}
