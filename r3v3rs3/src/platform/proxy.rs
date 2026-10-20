//! The proxies of the deployment platform. The platform sends them to the server as the snapshot
//! of the `Platform` discovery provider, so they are read-only and the server does not save them.
//! The platform sends them again after each start.

use crate::discovery::{DiscoveredProxy, DiscoverySnapshot, ProxyDefinition};
use r3v3rs3_api::container::ContainerName;
use r3v3rs3_api::discovery::{DiscoveryIssue, DiscoveryProvider, DiscoverySource, DiscoveryState};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppSpec, PlatformConfig};
use r3v3rs3_api::proxy::{HttpProxy, ProxyKind, Route, Server, ServerUrl};
use r3v3rs3_api::vhost::VirtualHost;

/// The name of the container of one deployment.
pub fn container_name(app: ShortId, deployment: ShortId) -> anyhow::Result<ContainerName> {
    Ok(format!("r3v3rs3-{app}-{deployment}").parse()?)
}

/// An app whose running containers listen on `host_ports` of 127.0.0.1.
pub struct LiveApp<'a> {
    pub id: ShortId,
    pub name: &'a str,
    pub spec: &'a AppSpec,
    pub host_ports: Vec<u16>,
}

/// The proxy of an app. An app without domains gets no proxy, because a proxy without virtual
/// hosts would answer every host name of its ports.
pub fn app_proxy(
    app: &LiveApp<'_>,
    config: &PlatformConfig,
) -> anyhow::Result<Option<DiscoveredProxy>> {
    if app.spec.domains.is_empty() || app.host_ports.is_empty() {
        return Ok(None);
    }
    let servers = app
        .host_ports
        .iter()
        .map(|port| {
            let url = format!("http://127.0.0.1:{port}/").parse::<ServerUrl>()?;
            Ok(Server::new(url))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let http = HttpProxy {
        vhosts: app
            .spec
            .domains
            .iter()
            .cloned()
            .map(VirtualHost::SubjectName)
            .collect(),
        routes: vec![Route {
            servers,
            ..Default::default()
        }],
        load_balancing: app.spec.load_balancing.unwrap_or_default(),
        sticky: app.spec.sticky.clone().unwrap_or_default(),
        ..Default::default()
    };
    let key = app.id.to_string();
    Ok(Some(DiscoveredProxy {
        key: key.clone(),
        source: DiscoverySource {
            provider: DiscoveryProvider::Platform,
            resource: app.name.to_string(),
        },
        definition: ProxyDefinition {
            key,
            name: app.name.to_string(),
            ports: if app.spec.ports.is_empty() {
                config.proxy_ports.clone()
            } else {
                app.spec.ports.clone()
            },
            active: true,
            acme: config.acme,
            kind: ProxyKind::Http(Box::new(http)),
        },
    }))
}

/// The snapshot of a successful read. The issues name the apps that got no proxy.
pub fn snapshot(proxies: Vec<DiscoveredProxy>, issues: Vec<DiscoveryIssue>) -> DiscoverySnapshot {
    DiscoverySnapshot {
        provider: DiscoveryProvider::Platform,
        generation: 0,
        state: DiscoveryState::Running,
        error: None,
        proxies: Some(proxies),
        certs: Vec::new(),
        issues,
    }
}

/// The snapshot of a failed read. The server keeps the proxies of the previous snapshot.
pub fn error_snapshot(error: String) -> DiscoverySnapshot {
    DiscoverySnapshot {
        provider: DiscoveryProvider::Platform,
        generation: 0,
        state: DiscoveryState::Error,
        error: Some(error),
        proxies: None,
        certs: Vec::new(),
        issues: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::platform::AppSource;

    fn spec(domains: &[&str]) -> AppSpec {
        AppSpec {
            source: AppSource::Image {
                image: "nginx:1.27".parse().unwrap(),
            },
            port: 80,
            domains: domains.iter().map(|d| d.parse().unwrap()).collect(),
            ports: Vec::new(),
            health_check_path: None,
            volumes: Vec::new(),
            restart: Default::default(),
            limits: Default::default(),
            host_port: None,
            database: None,
            uses_database: None,
            auto_deploy: false,
            require_approval: false,
            deploy_window: None,
            tag_trigger: None,
            store_app_id: None,
            targets: Vec::new(),
            load_balancing: None,
            alert_rules: None,
            sticky: None,
            git_config: None,
        }
    }

    #[test]
    fn an_app_proxy_routes_its_domains_to_the_published_port() -> anyhow::Result<()> {
        let shop_spec = spec(&["shop.example.com", "www.shop.example.com"]);
        let app = LiveApp {
            id: "abc-def".parse()?,
            name: "shop",
            spec: &shop_spec,
            host_ports: vec![49153],
        };
        let config = PlatformConfig {
            proxy_ports: vec!["http".into(), "https".into()],
            acme: Some("acm-bcd".parse()?),
            ..Default::default()
        };
        let proxy = app_proxy(&app, &config)?.expect("a proxy");
        assert_eq!(proxy.key, "abc-def");
        assert_eq!(proxy.source.resource, "shop");
        assert_eq!(proxy.definition.ports, ["http", "https"]);
        assert_eq!(proxy.definition.acme, config.acme);
        let ProxyKind::Http(http) = &proxy.definition.kind else {
            panic!("an HTTP proxy");
        };
        let vhosts = http
            .vhosts
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(vhosts, ["shop.example.com", "www.shop.example.com"]);
        assert_eq!(
            crate::discovery::first_route_servers(&proxy),
            ["http://127.0.0.1:49153/"]
        );
        Ok(())
    }

    #[test]
    fn an_app_proxy_with_multiple_targets_sets_load_balancing() -> anyhow::Result<()> {
        let mut shop_spec = spec(&["shop.example.com"]);
        shop_spec.load_balancing = Some(r3v3rs3_api::upstream::LoadBalancing::ClientIpHash);
        let app = LiveApp {
            id: "abc-def".parse()?,
            name: "shop",
            spec: &shop_spec,
            host_ports: vec![49153, 49154],
        };
        let config = PlatformConfig {
            proxy_ports: vec!["http".into()],
            ..Default::default()
        };
        let proxy = app_proxy(&app, &config)?.expect("a proxy");
        assert_eq!(
            crate::discovery::first_route_servers(&proxy),
            ["http://127.0.0.1:49153/", "http://127.0.0.1:49154/"]
        );
        let ProxyKind::Http(http) = &proxy.definition.kind else {
            panic!("an HTTP proxy");
        };
        assert_eq!(
            http.load_balancing,
            r3v3rs3_api::upstream::LoadBalancing::ClientIpHash
        );
        Ok(())
    }

    #[test]
    fn the_ports_of_the_app_win_over_the_proxy_ports_of_the_config() -> anyhow::Result<()> {
        let mut app_spec = spec(&["shop.example.com"]);
        app_spec.ports = vec!["https".into()];
        let app = LiveApp {
            id: "abc-def".parse()?,
            name: "shop",
            spec: &app_spec,
            host_ports: vec![49153],
        };
        let config = PlatformConfig {
            proxy_ports: vec!["http".into()],
            ..Default::default()
        };
        let proxy = app_proxy(&app, &config)?.expect("a proxy");
        assert_eq!(proxy.definition.ports, ["https"]);
        Ok(())
    }

    #[test]
    fn an_app_without_domains_gets_no_proxy() -> anyhow::Result<()> {
        let spec = spec(&[]);
        let app = LiveApp {
            id: "abc-def".parse()?,
            name: "shop",
            spec: &spec,
            host_ports: vec![49153],
        };
        assert!(app_proxy(&app, &PlatformConfig::default())?.is_none());
        Ok(())
    }

    #[test]
    fn a_container_name_joins_the_app_and_the_deployment() -> anyhow::Result<()> {
        let name = container_name("abc-def".parse()?, "ghj-klm".parse()?)?;
        assert_eq!(name.as_str(), "r3v3rs3-abc-def-ghj-klm");
        Ok(())
    }
}
