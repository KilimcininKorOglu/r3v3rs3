//! The Docker provider. It lists the running containers, reads their `r3v3rs3.*` labels and
//! follows the container events of the Docker Engine API.

use super::http::{read_json, ApiClient, Lines};
use super::labels;
use super::{Built, ProxyGroups, Reporter, Watch, DEBOUNCE, MIN_BACKOFF};
use anyhow::anyhow;
use r3v3rs3_api::discovery::{DiscoveryProvider, DockerDiscoveryConfig};
use serde_derive::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::{timeout, timeout_at, Instant};

const PROVIDER: DiscoveryProvider = DiscoveryProvider::Docker;
const CONTAINERS_PATH: &str = "/containers/json";
/// The container events: `filters={"type":["container"]}`.
const EVENTS_PATH: &str = "/events?filters=%7B%22type%22%3A%5B%22container%22%5D%7D";
/// The containers are read again after this time without events.
const RESYNC_INTERVAL: Duration = Duration::from_secs(300);
const COMPOSE_PROJECT: &str = "com.docker.compose.project";
const COMPOSE_SERVICE: &str = "com.docker.compose.service";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Container {
    #[serde(default)]
    id: String,
    #[serde(default)]
    names: Option<Vec<String>>,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    host_config: Option<HostConfig>,
    #[serde(default)]
    network_settings: Option<NetworkSettings>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HostConfig {
    #[serde(default)]
    network_mode: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct NetworkSettings {
    #[serde(default)]
    networks: Option<BTreeMap<String, Network>>,
}

#[derive(Debug, Default, Deserialize)]
struct Network {
    #[serde(rename = "IPAddress", default)]
    ip_address: String,
    #[serde(rename = "GlobalIPv6Address", default)]
    global_ipv6_address: String,
}

impl Container {
    fn name(&self) -> String {
        self.names
            .iter()
            .flatten()
            .next()
            .map(|name| name.trim_start_matches('/').to_string())
            .unwrap_or_else(|| self.id.chars().take(12).collect())
    }

    fn labels(&self) -> impl Iterator<Item = (&str, &str)> + Clone {
        self.labels
            .iter()
            .flatten()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    fn label(&self, key: &str) -> Option<&str> {
        self.labels.as_ref()?.get(key).map(String::as_str)
    }

    /// The replicas of a Compose service share their proxies.
    fn group(&self) -> String {
        match (self.label(COMPOSE_PROJECT), self.label(COMPOSE_SERVICE)) {
            (Some(project), Some(service)) => format!("{project}/{service}"),
            _ => self.name(),
        }
    }

    fn networks(&self) -> Vec<(&str, &Network)> {
        self.network_settings
            .iter()
            .filter_map(|settings| settings.networks.as_ref())
            .flatten()
            .map(|(name, network)| (name.as_str(), network))
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
struct Settings {
    network: String,
    exposed_by_default: bool,
}

impl Settings {
    fn selects(&self, container: &Container) -> bool {
        labels::selects(container.labels(), self.exposed_by_default)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Health {
    Ready,
    Starting,
    Unhealthy,
}

/// Reads the health from the status text of the container list, for example
/// `Up 5 minutes (healthy)`.
fn health(status: &str) -> Health {
    if status.contains("(unhealthy)") {
        Health::Unhealthy
    } else if status.contains("(health: starting)") {
        Health::Starting
    } else {
        Health::Ready
    }
}

/// The address of the container on the selected network.
fn address(container: &Container, network: &str) -> Result<String, String> {
    let host_mode = container
        .host_config
        .as_ref()
        .is_some_and(|config| config.network_mode == "host");
    if host_mode {
        return Ok("127.0.0.1".to_string());
    }
    let selected = select_network(&container.networks(), network)?;
    [&selected.ip_address, &selected.global_ipv6_address]
        .into_iter()
        .find(|ip| !ip.is_empty())
        .cloned()
        .ok_or_else(|| "the container has no IP address".to_string())
}

fn select_network<'a>(networks: &[(&str, &'a Network)], name: &str) -> Result<&'a Network, String> {
    if !name.is_empty() {
        return networks
            .iter()
            .find(|(network, _)| *network == name)
            .map(|(_, network)| *network)
            .ok_or_else(|| format!("the container is not on the network {name}"));
    }
    match networks {
        [(_, network)] => Ok(network),
        [] => Err("the container has no network".to_string()),
        _ => Err(
            "the container is on more than one network, so the Docker network setting is required"
                .to_string(),
        ),
    }
}

fn build(mut containers: Vec<Container>, settings: &Settings) -> Built {
    containers.sort_by_key(Container::name);
    let mut groups = ProxyGroups::new(PROVIDER);
    let mut built = Built::default();
    for container in containers.iter().filter(|c| settings.selects(c)) {
        let name = container.name();
        match container_definitions(container, settings) {
            Ok(parsed) => built.add_parsed(&mut groups, &container.group(), &name, parsed),
            Err(Some(message)) => built.issue(&name, message),
            Err(None) => {}
        }
    }
    built.proxies = groups.into_proxies();
    built
}

/// The proxy definitions and the label issues of a container. `Err(None)` skips a container whose
/// health check has not passed yet.
fn container_definitions(
    container: &Container,
    settings: &Settings,
) -> Result<labels::Parsed, Option<String>> {
    match health(&container.status) {
        Health::Starting => return Err(None),
        Health::Unhealthy => return Err(Some("the container is unhealthy".to_string())),
        Health::Ready => {}
    }
    let host = address(container, &settings.network).map_err(Some)?;
    Ok(labels::parse(container.labels(), Some(&host)))
}

pub(super) struct Provider {
    client: ApiClient,
    settings: Settings,
    reporter: Reporter,
}

#[async_trait::async_trait]
impl Watch for Provider {
    fn reporter(&self) -> &Reporter {
        &self.reporter
    }

    async fn watch(&self, backoff: &mut Duration) -> anyhow::Result<std::convert::Infallible> {
        self.follow(backoff).await?;
        Err(anyhow!("the Docker event stream closed"))
    }
}

impl Provider {
    pub(super) fn new(
        config: &DockerDiscoveryConfig,
        client: ApiClient,
        reporter: Reporter,
    ) -> Self {
        let settings = Settings {
            network: config.network.trim().to_string(),
            exposed_by_default: config.exposed_by_default,
        };
        Self {
            client,
            settings,
            reporter,
        }
    }

    /// Opens the event stream, reads the containers and reads them again after each burst of
    /// events. Returns when the stream ends.
    async fn follow(&self, backoff: &mut Duration) -> anyhow::Result<()> {
        let mut events = Lines::new(self.client.send(self.client.get(EVENTS_PATH)?).await?);
        self.sync().await?;
        *backoff = MIN_BACKOFF;
        loop {
            if let Ok(line) = timeout(RESYNC_INTERVAL, events.next()).await {
                if line?.is_none() || !debounce(&mut events).await? {
                    return Ok(());
                }
            }
            self.sync().await?;
        }
    }

    async fn sync(&self) -> anyhow::Result<()> {
        let response = self.client.send(self.client.get(CONTAINERS_PATH)?).await?;
        let built = build(read_json(response).await?, &self.settings);
        self.reporter.running(built).await
    }
}

/// Reads the events that follow within the debounce time. Returns false when the stream ends.
async fn debounce(events: &mut Lines) -> anyhow::Result<bool> {
    let deadline = Instant::now() + DEBOUNCE;
    while let Ok(line) = timeout_at(deadline, events.next()).await {
        if line?.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::first_route_servers as servers;
    use r3v3rs3_api::discovery::DiscoveryIssue;
    use serde_json::json;

    fn containers(value: serde_json::Value) -> Vec<Container> {
        serde_json::from_value(value).unwrap()
    }

    fn container(
        name: &str,
        labels: serde_json::Value,
        networks: serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "Id": format!("{name}0123456789"),
            "Names": [format!("/{name}")],
            "Labels": labels,
            "Status": "Up 5 minutes",
            "HostConfig": {"NetworkMode": "bridge"},
            "NetworkSettings": {"Networks": networks},
        })
    }

    #[test]
    fn compose_replicas_share_one_proxy() {
        let labels = json!({
            "r3v3rs3.enable": "true",
            "r3v3rs3.http.app.ports": "http",
            "r3v3rs3.http.app.port": "8080",
            COMPOSE_PROJECT: "shop",
            COMPOSE_SERVICE: "web",
        });
        let list = containers(json!([
            container(
                "shop-web-2",
                labels.clone(),
                json!({"shop_default": {"IPAddress": "172.18.0.3"}})
            ),
            container(
                "shop-web-1",
                labels,
                json!({"shop_default": {"IPAddress": "172.18.0.2"}})
            ),
        ]));
        let built = build(list, &Settings::default());
        assert!(built.issues.is_empty(), "{:?}", built.issues);
        assert_eq!(built.proxies.len(), 1);
        let proxy = &built.proxies[0];
        assert_eq!(proxy.key, "shop/web/http.app");
        assert_eq!(proxy.source.resource, "shop-web-1, shop-web-2");
        assert_eq!(
            servers(proxy),
            ["http://172.18.0.2:8080/", "http://172.18.0.3:8080/"]
        );
    }

    #[test]
    fn containers_are_selected_by_the_enable_label() {
        let definition = json!({"r3v3rs3.http.app.ports": "http", "r3v3rs3.http.app.port": "80"});
        let mut disabled = definition.clone();
        disabled["r3v3rs3.enable"] = json!("false");
        let network = json!({"bridge": {"IPAddress": "172.17.0.2"}});
        let list = || {
            containers(json!([
                container("implicit", definition.clone(), network.clone()),
                container("disabled", disabled.clone(), network.clone()),
                container(
                    "unlabeled",
                    json!({"traefik.enable": "true"}),
                    network.clone()
                ),
            ]))
        };

        assert!(build(list(), &Settings::default()).proxies.is_empty());

        let settings = Settings {
            exposed_by_default: true,
            ..Default::default()
        };
        let built = build(list(), &settings);
        let resources = built
            .proxies
            .iter()
            .map(|proxy| proxy.source.resource.as_str())
            .collect::<Vec<_>>();
        assert_eq!(resources, ["implicit"]);
    }

    #[test]
    fn the_address_comes_from_the_selected_network() {
        let labels = json!({
            "r3v3rs3.enable": "true",
            "r3v3rs3.http.app.ports": "http",
            "r3v3rs3.http.app.port": "80",
        });
        let two_networks = json!({
            "front": {"IPAddress": "10.0.1.2"},
            "back": {"IPAddress": "", "GlobalIPv6Address": "fd00::2"},
        });
        let mut host = container("host", labels.clone(), json!({}));
        host["HostConfig"]["NetworkMode"] = json!("host");
        let mut unhealthy = container(
            "unhealthy",
            labels.clone(),
            json!({"bridge": {"IPAddress": "172.17.0.9"}}),
        );
        unhealthy["Status"] = json!("Up 1 minute (unhealthy)");
        let mut starting = container(
            "starting",
            labels.clone(),
            json!({"bridge": {"IPAddress": "172.17.0.8"}}),
        );
        starting["Status"] = json!("Up 1 second (health: starting)");
        let list = || {
            containers(json!([
                container("multi", labels.clone(), two_networks.clone()),
                host.clone(),
                unhealthy.clone(),
                starting.clone(),
            ]))
        };

        let built = build(list(), &Settings::default());
        let proxies = built
            .proxies
            .iter()
            .map(|proxy| (proxy.source.resource.as_str(), servers(proxy)))
            .collect::<Vec<_>>();
        assert_eq!(proxies, [("host", vec!["http://127.0.0.1/".to_string()])]);
        let issues = built
            .issues
            .iter()
            .map(|issue| format!("{}: {}", issue.resource, issue.message))
            .collect::<Vec<_>>();
        assert_eq!(
            issues,
            [
                "multi: the container is on more than one network, so the Docker network setting is required",
                "unhealthy: the container is unhealthy",
            ]
        );

        let settings = Settings {
            network: "back".into(),
            ..Default::default()
        };
        let built = build(list(), &settings);
        let multi = built
            .proxies
            .iter()
            .find(|proxy| proxy.source.resource == "multi")
            .map(servers);
        assert_eq!(multi, Some(vec!["http://[fd00::2]/".to_string()]));
    }

    #[test]
    fn label_issues_name_the_container() {
        let list = containers(json!([container(
            "web",
            json!({"r3v3rs3.enable": "true", "r3v3rs3.http.app.port": "80"}),
            json!({"bridge": {"IPAddress": "172.17.0.2"}}),
        )]));
        let built = build(list, &Settings::default());
        assert_eq!(
            built.issues,
            [DiscoveryIssue {
                resource: "web".into(),
                message: "http.app: ports is required".into(),
            }]
        );
    }

    #[test]
    fn a_container_list_with_null_fields_parses() {
        let list = containers(json!([{
            "Id": "abcdef0123456789",
            "Names": null,
            "Labels": null,
            "Status": "Up",
            "NetworkSettings": {"Networks": null},
        }]));
        assert_eq!(list[0].name(), "abcdef012345");
        assert!(list[0].networks().is_empty());
    }
}
