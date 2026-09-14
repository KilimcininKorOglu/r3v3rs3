//! The Consul provider. It reads the `r3v3rs3.*` tags of the passing service instances in the
//! catalog and the keys under a prefix in the key-value store, and follows their changes with
//! blocking queries.

use super::kv::{self, KvEntry};
use super::labels::{self, Upstream};
use super::{Built, ProxyGroups, Reporter, Watch, DEBOUNCE, MIN_BACKOFF};
use crate::kv::consul::{self, ConsulClient, WAIT};
use crate::kv::http::{read_json, ApiClient};
use anyhow::anyhow;
use futures::future::{select_all, BoxFuture, FutureExt};
use r3v3rs3_api::discovery::{ConsulDiscoveryConfig, DiscoveryProvider};
use serde_derive::Deserialize;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::time::Duration;

const PROVIDER: DiscoveryProvider = DiscoveryProvider::Consul;

/// A resource that a blocking query follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Watched {
    /// The service names and their tags.
    Services,
    /// Every health check, so that a change of an instance health is seen.
    Health,
    Kv,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ServiceEntry {
    #[serde(default)]
    node: NodeInfo,
    #[serde(default)]
    service: ServiceInfo,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct NodeInfo {
    #[serde(default)]
    address: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ServiceInfo {
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    address: String,
    #[serde(default)]
    port: u16,
}

#[derive(Debug, Clone, Default)]
struct Settings {
    datacenter: String,
    catalog: bool,
    kv: bool,
    prefix: String,
    exposed_by_default: bool,
}

impl Settings {
    fn new(config: &ConsulDiscoveryConfig) -> Self {
        Self {
            datacenter: config.datacenter.trim().to_string(),
            catalog: config.catalog,
            kv: config.kv,
            prefix: config.prefix.trim_matches('/').to_string(),
            exposed_by_default: config.exposed_by_default,
        }
    }

    /// The path and the query of an API request. The datacenter is added to the query.
    fn path(&self, segments: &[&str], query: &[(&str, &str)]) -> String {
        consul::path(&self.datacenter, segments, query)
    }

    fn watched_path(&self, watched: Watched, query: &[(&str, &str)]) -> String {
        match watched {
            Watched::Services => self.path(&["v1", "catalog", "services"], query),
            Watched::Health => self.path(&["v1", "health", "state", "any"], query),
            Watched::Kv => {
                let mut segments = vec!["v1", "kv"];
                segments.extend(self.prefix.split('/'));
                segments.push("");
                let query = [&[("recurse", "true")], query].concat();
                self.path(&segments, &query)
            }
        }
    }
}

/// The `r3v3rs3.` tags of an instance as labels, and one issue for each tag without a value.
fn tag_labels<'a>(
    tags: impl Iterator<Item = &'a String>,
) -> (Vec<(&'a str, &'a str)>, Vec<String>) {
    let mut pairs = Vec::new();
    let mut issues = Vec::new();
    for tag in tags {
        match tag.split_once('=') {
            Some((key, value)) => pairs.push((key.trim(), value.trim())),
            None if labels::is_definition(tag) || tag == labels::ENABLE => {
                issues.push(format!("the tag has no value: {tag}"));
            }
            None => {}
        }
    }
    (pairs, issues)
}

/// Adds the proxies of the passing instances of a selected service. The instances of a service
/// share its proxies.
fn add_service(
    built: &mut Built,
    groups: &mut ProxyGroups,
    name: &str,
    entries: &[ServiceEntry],
    exposed_by_default: bool,
) {
    if entries.is_empty() {
        return built.issue(name, "the service has no passing instance".to_string());
    }
    for entry in entries {
        let (pairs, issues) = tag_labels(entry.service.tags.iter().flatten());
        if !labels::selects(pairs.iter().copied(), exposed_by_default) {
            continue;
        }
        for message in issues {
            built.issue(name, message);
        }
        let service = &entry.service;
        let host = [&service.address, &entry.node.address]
            .into_iter()
            .find(|address| !address.is_empty());
        let upstream = Upstream {
            host: host.map(String::as_str),
            port: Some(service.port).filter(|port| *port != 0),
        };
        built.add_parsed(groups, name, name, labels::parse_at(pairs, upstream));
    }
}

pub(super) struct Provider {
    client: ConsulClient,
    settings: Settings,
    reporter: Reporter,
}

type Blocking<'a> = BoxFuture<'a, (Watched, anyhow::Result<u64>)>;

#[async_trait::async_trait]
impl Watch for Provider {
    fn reporter(&self) -> &Reporter {
        &self.reporter
    }

    async fn watch(&self, backoff: &mut Duration) -> anyhow::Result<Infallible> {
        self.follow(backoff).await
    }
}

impl Provider {
    pub(super) fn new(
        config: &ConsulDiscoveryConfig,
        client: ApiClient,
        reporter: Reporter,
    ) -> Self {
        Self {
            client: ConsulClient::new(client, config.token.as_deref()),
            settings: Settings::new(config),
            reporter,
        }
    }

    /// Reads every resource, then waits until the index of a resource changes and reads again.
    async fn follow(&self, backoff: &mut Duration) -> anyhow::Result<Infallible> {
        loop {
            let indexes = self.sync().await?;
            *backoff = MIN_BACKOFF;
            let mut pending = indexes
                .iter()
                .map(|(watched, index)| self.block(*watched, *index))
                .collect::<Vec<_>>();
            loop {
                if pending.is_empty() {
                    return Err(anyhow!("no Consul resource is read"));
                }
                let ((watched, result), _, rest) = select_all(pending).await;
                pending = rest;
                let index = result?;
                if !indexes.contains(&(watched, index)) {
                    break;
                }
                pending.push(self.block(watched, index));
            }
            tokio::time::sleep(DEBOUNCE).await;
        }
    }

    /// Reads every resource and sends the proxies. Returns the index of each resource.
    async fn sync(&self) -> anyhow::Result<Vec<(Watched, u64)>> {
        let mut built = Built::default();
        let mut groups = ProxyGroups::new(PROVIDER);
        let mut indexes = Vec::new();
        if self.settings.catalog {
            indexes.extend(self.read_catalog(&mut built, &mut groups).await?);
        }
        if self.settings.kv {
            indexes.push(self.read_kv(&mut built, &mut groups).await?);
        }
        built.proxies = groups.into_proxies();
        self.reporter.running(built).await?;
        Ok(indexes)
    }

    /// Reads the selected services. The health index is read first, so a later change of an
    /// instance is not missed.
    async fn read_catalog(
        &self,
        built: &mut Built,
        groups: &mut ProxyGroups,
    ) -> anyhow::Result<[(Watched, u64); 2]> {
        let settings = &self.settings;
        let (_, health_index) = self
            .client
            .get(&settings.watched_path(Watched::Health, &[]))
            .await?;
        let path = settings.watched_path(Watched::Services, &[]);
        let (response, services_index) = self.client.get(&path).await?;
        let services: BTreeMap<String, Option<Vec<String>>> = read_json(response).await?;
        for (name, tags) in &services {
            let (pairs, _) = tag_labels(tags.iter().flatten());
            if !labels::selects(pairs.iter().copied(), settings.exposed_by_default) {
                continue;
            }
            let path = settings.path(&["v1", "health", "service", name], &[("passing", "true")]);
            let (response, _) = self.client.get(&path).await?;
            let entries: Vec<ServiceEntry> = read_json(response).await?;
            add_service(built, groups, name, &entries, settings.exposed_by_default);
        }
        Ok([
            (Watched::Health, health_index),
            (Watched::Services, services_index),
        ])
    }

    async fn read_kv(
        &self,
        built: &mut Built,
        groups: &mut ProxyGroups,
    ) -> anyhow::Result<(Watched, u64)> {
        let path = self.settings.watched_path(Watched::Kv, &[]);
        let (entries, index) = self.client.list(&path).await?;
        let entries = entries
            .into_iter()
            .map(|entry| KvEntry {
                key: entry.key,
                value: entry.value,
            })
            .collect::<Vec<_>>();
        kv::add_kv(built, groups, &entries, &self.settings.prefix);
        Ok((Watched::Kv, index))
    }

    /// Waits until the index of the resource passes `index` or the wait time ends, and returns
    /// the new index.
    fn block(&self, watched: Watched, index: u64) -> Blocking<'_> {
        async move {
            let index = index.to_string();
            let query = [("index", index.as_str()), ("wait", WAIT)];
            let path = self.settings.watched_path(watched, &query);
            (watched, self.client.block(&path).await)
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::first_route_servers as servers;
    use serde_json::json;

    fn instances(value: serde_json::Value) -> Vec<ServiceEntry> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn instances_of_a_service_share_its_proxy() {
        let tags = json!([
            "primary",
            "r3v3rs3.enable=true",
            "r3v3rs3.http.app.ports=http"
        ]);
        let entries = instances(json!([
            {"Node": {"Address": "10.0.0.1"}, "Service": {"Tags": tags, "Address": "", "Port": 8080}},
            {"Node": {"Address": "10.0.0.9"}, "Service": {"Tags": tags, "Address": "10.0.0.2", "Port": 8080}},
            {"Node": {"Address": "10.0.0.3"}, "Service": {"Tags": ["r3v3rs3.http.app.ports=http"], "Port": 8080}},
        ]));
        let mut built = Built::default();
        let mut groups = ProxyGroups::new(PROVIDER);
        add_service(&mut built, &mut groups, "web", &entries, false);
        let proxies = groups.into_proxies();
        assert!(built.issues.is_empty(), "{:?}", built.issues);
        assert_eq!(proxies.len(), 1);
        assert_eq!(proxies[0].key, "web/http.app");
        assert_eq!(proxies[0].source.resource, "web");
        assert_eq!(
            servers(&proxies[0]),
            ["http://10.0.0.1:8080/", "http://10.0.0.2:8080/"]
        );

        let mut built = Built::default();
        let mut groups = ProxyGroups::new(PROVIDER);
        add_service(&mut built, &mut groups, "api", &[], false);
        let issues = built
            .issues
            .iter()
            .map(|issue| format!("{}: {}", issue.resource, issue.message))
            .collect::<Vec<_>>();
        assert_eq!(issues, ["api: the service has no passing instance"]);
    }

    #[test]
    fn tags_without_a_value_are_issues() {
        let tags = [
            "r3v3rs3.enable".to_string(),
            "v1".to_string(),
            "a=b".to_string(),
        ];
        let (pairs, issues) = tag_labels(tags.iter());
        assert_eq!(pairs, [("a", "b")]);
        assert_eq!(issues, ["the tag has no value: r3v3rs3.enable"]);
    }

    #[test]
    fn watched_paths_add_the_prefix_and_the_datacenter() {
        let settings = Settings::new(&ConsulDiscoveryConfig {
            datacenter: "eu west".into(),
            prefix: "/apps/r3v3rs3/".into(),
            ..Default::default()
        });
        assert_eq!(
            settings.watched_path(Watched::Kv, &[("index", "7"), ("wait", WAIT)]),
            "/v1/kv/apps/r3v3rs3/?recurse=true&index=7&wait=5m&dc=eu%20west"
        );
        assert_eq!(
            Settings::default().watched_path(Watched::Services, &[]),
            "/v1/catalog/services"
        );
    }
}
