//! The server side of service discovery: the latest snapshot of each provider, the proxies that
//! the server builds from the snapshots and the provider statuses.

use super::cert_list::CertList;
use super::credentials::seal;
use super::proxy_list::accepts;
use crate::discovery::{ids, DiscoveredProxy, DiscoverySnapshot};
use crate::proxy::tls::upstream_client_config;
use hyper::header::HeaderValue;
use r3v3rs3_api::discovery::{
    ConsulDiscoveryConfig, DiscoveryConfig, Endpoint, EtcdDiscoveryConfig,
};
use r3v3rs3_api::discovery::{DiscoveryIssue, DiscoveryProvider, DiscoveryState, DiscoveryStatus};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry, ProxyKind};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::SystemTime;
use tokio::task::JoinHandle;

#[derive(Debug, Default)]
pub struct DiscoveryRegistry {
    providers: BTreeMap<DiscoveryProvider, ProviderRecord>,
}

#[derive(Debug)]
struct ProviderRecord {
    state: DiscoveryState,
    error: Option<String>,
    proxies: Vec<DiscoveredProxy>,
    provider_issues: Vec<DiscoveryIssue>,
    build_issues: Vec<DiscoveryIssue>,
    added: usize,
    updated_at: i64,
    /// Sealed proxies by the JSON text of the proxy before sealing. A definition that does not
    /// change keeps its password hashes, so the proxy does not change on every read.
    sealed: HashMap<String, Proxy>,
}

impl ProviderRecord {
    fn new() -> Self {
        Self {
            state: DiscoveryState::Connecting,
            error: None,
            proxies: Vec::new(),
            provider_issues: Vec::new(),
            build_issues: Vec::new(),
            added: 0,
            updated_at: 0,
            sealed: HashMap::new(),
        }
    }
}

impl DiscoveryRegistry {
    pub fn update(&mut self, snapshot: DiscoverySnapshot) {
        let record = self
            .providers
            .entry(snapshot.provider)
            .or_insert_with(ProviderRecord::new);
        record.state = snapshot.state;
        record.error = snapshot.error;
        record.provider_issues = snapshot.issues;
        record.updated_at = unix_now();
        if let Some(proxies) = snapshot.proxies {
            record.proxies = proxies;
        }
    }

    pub fn contains(&self, provider: DiscoveryProvider) -> bool {
        self.providers.contains_key(&provider)
    }

    pub fn remove(&mut self, provider: DiscoveryProvider) {
        self.providers.remove(&provider);
    }

    pub fn providers(&self) -> Vec<DiscoveryProvider> {
        self.providers.keys().copied().collect()
    }

    pub fn proxies(&self, provider: DiscoveryProvider) -> Vec<DiscoveredProxy> {
        self.providers
            .get(&provider)
            .map(|record| record.proxies.clone())
            .unwrap_or_default()
    }

    /// Replaces the credentials of the proxies with hashes. A proxy whose definition did not
    /// change keeps its previous hashes.
    pub async fn seal(
        &mut self,
        provider: DiscoveryProvider,
        entries: Vec<ProxyEntry>,
    ) -> (Vec<ProxyEntry>, Vec<DiscoveryIssue>) {
        let mut previous = self
            .providers
            .get_mut(&provider)
            .map(|record| std::mem::take(&mut record.sealed))
            .unwrap_or_default();
        let mut current = HashMap::new();
        let mut sealed = Vec::new();
        let mut issues = Vec::new();
        for mut entry in entries {
            match seal_cached(&mut previous, &mut current, entry.proxy.clone()).await {
                Ok(proxy) => {
                    entry.proxy = proxy;
                    sealed.push(entry);
                }
                Err(err) => issues.push(entry_issue(&entry, err.to_string())),
            }
        }
        if let Some(record) = self.providers.get_mut(&provider) {
            record.sealed = current;
        }
        (sealed, issues)
    }

    pub fn set_result(
        &mut self,
        provider: DiscoveryProvider,
        added: usize,
        issues: Vec<DiscoveryIssue>,
    ) {
        if let Some(record) = self.providers.get_mut(&provider) {
            record.added = added;
            record.build_issues = issues;
        }
    }

    pub fn statuses(&self) -> Vec<DiscoveryStatus> {
        self.providers
            .iter()
            .map(|(provider, record)| DiscoveryStatus {
                provider: *provider,
                state: record.state,
                error: record.error.clone(),
                proxies: record.added,
                issues: record
                    .provider_issues
                    .iter()
                    .chain(&record.build_issues)
                    .cloned()
                    .collect(),
                updated_at: record.updated_at,
            })
            .collect()
    }
}

/// The running provider tasks. A snapshot of a stopped task is ignored, because it can arrive
/// after the task stopped.
#[derive(Debug, Default)]
pub struct DiscoveryTasks {
    tasks: BTreeMap<DiscoveryProvider, ProviderTask>,
    generation: u64,
}

#[derive(Debug)]
struct ProviderTask {
    generation: u64,
    handle: Option<JoinHandle<()>>,
}

impl DiscoveryTasks {
    /// Whether a snapshot comes from the running task of the provider. A provider that the server
    /// never started accepts every snapshot.
    pub fn accepts(&self, provider: DiscoveryProvider, generation: u64) -> bool {
        self.tasks
            .get(&provider)
            .is_none_or(|task| task.handle.is_some() && task.generation == generation)
    }

    /// Stops the running task and starts a new one with the next generation.
    pub fn start(
        &mut self,
        provider: DiscoveryProvider,
        spawn: impl FnOnce(u64) -> JoinHandle<()>,
    ) {
        self.stop(provider);
        self.generation += 1;
        let generation = self.generation;
        let handle = Some(spawn(generation));
        self.tasks
            .insert(provider, ProviderTask { generation, handle });
    }

    /// Returns true when a task was running.
    pub fn stop(&mut self, provider: DiscoveryProvider) -> bool {
        self.tasks
            .get_mut(&provider)
            .and_then(|task| task.handle.take())
            .map(|handle| handle.abort())
            .is_some()
    }
}

impl Drop for DiscoveryTasks {
    fn drop(&mut self) {
        for handle in self
            .tasks
            .values_mut()
            .filter_map(|task| task.handle.take())
        {
            handle.abort();
        }
    }
}

/// Checks the settings of the enabled providers.
pub fn validate_config(config: &DiscoveryConfig, certs: &CertList) -> Result<(), Error> {
    for provider in DiscoveryProvider::ALL {
        if let Some((endpoints, client_cert)) = api_settings(config, provider) {
            for endpoint in endpoints {
                parse_endpoint(endpoint)?;
            }
            upstream_client_config(certs, client_cert)?;
        }
    }
    validate_consul(&config.consul)?;
    validate_etcd(&config.etcd)
}

/// The API addresses and the client certificate of an enabled provider that reads an HTTP API.
pub fn api_settings(
    config: &DiscoveryConfig,
    provider: DiscoveryProvider,
) -> Option<(Vec<&str>, Option<ShortId>)> {
    match provider {
        DiscoveryProvider::Docker if config.docker.enabled => Some((
            vec![config.docker.endpoint.as_str()],
            config.docker.client_cert,
        )),
        DiscoveryProvider::Consul if config.consul.enabled => Some((
            vec![config.consul.address.as_str()],
            config.consul.client_cert,
        )),
        DiscoveryProvider::Etcd if config.etcd.enabled => Some((
            config.etcd.endpoints.iter().map(String::as_str).collect(),
            config.etcd.client_cert,
        )),
        _ => None,
    }
}

fn validate_etcd(etcd: &EtcdDiscoveryConfig) -> Result<(), Error> {
    let has_user = !etcd.username.trim().is_empty();
    let reason = if !etcd.enabled {
        return Ok(());
    } else if etcd.endpoints.is_empty() {
        "etcd needs at least one endpoint"
    } else if etcd.prefix.trim_matches('/').is_empty() {
        "the etcd key prefix is empty"
    } else if has_user != etcd.password.is_some() {
        "the etcd user name and password must be set together"
    } else {
        return Ok(());
    };
    Err(Error::InvalidDiscoveryConfig {
        reason: reason.to_string(),
    })
}

fn validate_consul(consul: &ConsulDiscoveryConfig) -> Result<(), Error> {
    let reason = if !consul.enabled {
        return Ok(());
    } else if !consul.catalog && !consul.kv {
        "Consul needs the catalog or the key-value store"
    } else if consul.kv && consul.prefix.trim_matches('/').is_empty() {
        "the Consul key prefix is empty"
    } else if consul
        .token
        .as_deref()
        .is_some_and(|token| HeaderValue::from_str(token).is_err())
    {
        "the Consul token contains characters that a header cannot hold"
    } else {
        return Ok(());
    };
    Err(Error::InvalidDiscoveryConfig {
        reason: reason.to_string(),
    })
}

pub fn parse_endpoint(endpoint: &str) -> Result<Endpoint, Error> {
    let endpoint = endpoint.parse::<Endpoint>()?;
    if cfg!(not(unix)) && matches!(endpoint, Endpoint::Unix(_)) {
        return Err(Error::InvalidDiscoveryConfig {
            reason: "Unix sockets are not available on this platform".to_string(),
        });
    }
    Ok(endpoint)
}

/// The providers whose settings differ.
pub fn changed_providers(old: &DiscoveryConfig, new: &DiscoveryConfig) -> Vec<DiscoveryProvider> {
    let mut changed = Vec::new();
    if old.docker != new.docker {
        changed.push(DiscoveryProvider::Docker);
    }
    if old.consul != new.consul {
        changed.push(DiscoveryProvider::Consul);
    }
    if old.etcd != new.etcd {
        changed.push(DiscoveryProvider::Etcd);
    }
    changed
}

async fn seal_cached(
    previous: &mut HashMap<String, Proxy>,
    current: &mut HashMap<String, Proxy>,
    proxy: Proxy,
) -> Result<Proxy, Error> {
    let key = serde_json::to_string(&proxy).map_err(|_| Error::FailedToHashPassword)?;
    let sealed = match previous.remove(&key) {
        Some(sealed) => sealed,
        None => seal(proxy).await?,
    };
    current.insert(key, sealed.clone());
    Ok(sealed)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |time| i64::try_from(time.as_secs()).unwrap_or(i64::MAX))
}

/// The proxies of one provider with their ports resolved and their ids assigned, and one issue
/// for each definition that did not become a proxy.
#[derive(Debug, Default)]
pub struct Built {
    pub entries: Vec<ProxyEntry>,
    pub issues: Vec<DiscoveryIssue>,
}

/// Builds the proxies of a provider. `taken` holds the ids of the resources that the provider
/// does not own.
pub fn build(
    provider: DiscoveryProvider,
    proxies: &[DiscoveredProxy],
    ports: &[PortEntry],
    mut taken: HashSet<ShortId>,
    validate: impl Fn(&Proxy) -> Result<(), Error>,
) -> Built {
    let mut built = Built::default();
    let mut keys = HashSet::new();
    for proxy in proxies {
        let result = if keys.insert(proxy.key.as_str()) {
            build_proxy(proxy, ports, &validate)
        } else {
            Err("the proxy is defined more than once".to_string())
        };
        match result {
            Ok(value) => {
                let id = ids::discovered_id(provider, &proxy.key, &taken);
                taken.insert(id);
                built.entries.push(ProxyEntry {
                    id,
                    proxy: value,
                    source: Some(proxy.source.clone()),
                });
            }
            Err(message) => built.issues.push(DiscoveryIssue {
                resource: proxy.source.resource.clone(),
                message: format!("{}: {message}", proxy.definition.key),
            }),
        }
    }
    built
}

fn build_proxy(
    proxy: &DiscoveredProxy,
    ports: &[PortEntry],
    validate: &impl Fn(&Proxy) -> Result<(), Error>,
) -> Result<Proxy, String> {
    let definition = &proxy.definition;
    let value = Proxy {
        active: definition.active,
        name: definition.name.clone(),
        ports: resolve_ports(&definition.ports, &definition.kind, ports)?,
        kind: definition.kind.clone(),
    };
    validate(&value).map_err(|err| err.to_string())?;
    Ok(value)
}

/// Finds each port by its name or its id. A name must select exactly one port, and the port must
/// accept the protocol of the proxy.
pub fn resolve_ports(
    names: &[String],
    kind: &ProxyKind,
    ports: &[PortEntry],
) -> Result<Vec<ShortId>, String> {
    names
        .iter()
        .map(|name| resolve_port(name, kind, ports))
        .collect()
}

fn resolve_port(name: &str, kind: &ProxyKind, ports: &[PortEntry]) -> Result<ShortId, String> {
    let mut matches = ports
        .iter()
        .filter(|entry| entry.port.name == name || entry.id.to_string() == name);
    let port = matches
        .next()
        .ok_or_else(|| format!("port not found: {name}"))?;
    if matches.next().is_some() {
        return Err(format!("more than one port has the name: {name}"));
    }
    if !accepts(kind, &port.port.listen) {
        return Err(format!("port {name} does not accept this protocol"));
    }
    Ok(port.id)
}

fn entry_issue(entry: &ProxyEntry, message: String) -> DiscoveryIssue {
    DiscoveryIssue {
        resource: entry
            .source
            .as_ref()
            .map(|source| source.resource.clone())
            .unwrap_or_default(),
        message: format!("{}: {message}", entry.proxy.name),
    }
}

/// One issue for each proxy that the proxy list did not add.
pub fn conflict_issues(entries: &[ProxyEntry], skipped: &[ShortId]) -> Vec<DiscoveryIssue> {
    entries
        .iter()
        .filter(|entry| skipped.contains(&entry.id))
        .map(|entry| {
            entry_issue(
                entry,
                "the proxy uses a TCP port of another TCP proxy".to_string(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::ProxyDefinition;
    use r3v3rs3_api::discovery::DiscoverySource;
    use r3v3rs3_api::port::Port;
    use r3v3rs3_api::proxy::{HttpProxy, TcpProxy};

    fn port(id: &str, name: &str, listen: &str) -> PortEntry {
        PortEntry {
            id: id.parse().unwrap(),
            port: Port {
                active: true,
                name: name.into(),
                listen: listen.parse().unwrap(),
                opts: Default::default(),
            },
        }
    }

    fn discovered(key: &str, ports: &[&str], kind: ProxyKind) -> DiscoveredProxy {
        DiscoveredProxy {
            key: key.into(),
            source: DiscoverySource {
                provider: DiscoveryProvider::Docker,
                resource: "web-1".into(),
            },
            definition: ProxyDefinition {
                key: "http.app".into(),
                name: "app".into(),
                ports: ports.iter().map(|port| port.to_string()).collect(),
                active: true,
                kind,
            },
        }
    }

    fn http() -> ProxyKind {
        ProxyKind::Http(Box::<HttpProxy>::default())
    }

    #[test]
    fn ports_are_found_by_a_unique_name_or_an_id() {
        let ports = [
            port("p1", "http", "/ip4/0.0.0.0/tcp/80/http"),
            port("p2", "tcp", "/ip4/0.0.0.0/tcp/5432"),
            port("p3", "twin", "/ip4/0.0.0.0/tcp/81/http"),
            port("p4", "twin", "/ip4/0.0.0.0/tcp/82/http"),
        ];
        let names = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let ids = resolve_ports(&names(&["http", "p3"]), &http(), &ports).unwrap();
        assert_eq!(ids, ["p1".parse().unwrap(), "p3".parse().unwrap()]);

        assert_eq!(
            resolve_ports(&names(&["https"]), &http(), &ports),
            Err("port not found: https".to_string())
        );
        assert_eq!(
            resolve_ports(&names(&["twin"]), &http(), &ports),
            Err("more than one port has the name: twin".to_string())
        );
        assert_eq!(
            resolve_ports(&names(&["tcp"]), &http(), &ports),
            Err("port tcp does not accept this protocol".to_string())
        );
        let tcp = ProxyKind::Tcp(TcpProxy::default());
        assert!(resolve_ports(&names(&["tcp"]), &tcp, &ports).is_ok());
    }

    #[test]
    fn build_assigns_ids_and_reports_invalid_definitions() {
        let ports = [port("p1", "http", "/ip4/0.0.0.0/tcp/80/http")];
        let proxies = [
            discovered("a", &["http"], http()),
            discovered("a", &["http"], http()),
            discovered("b", &["missing"], http()),
            discovered("c", &["http"], http()),
        ];
        let built = build(
            DiscoveryProvider::Docker,
            &proxies,
            &ports,
            HashSet::new(),
            |_| Ok(()),
        );
        assert_eq!(built.entries.len(), 2);
        assert_ne!(built.entries[0].id, built.entries[1].id);
        assert_eq!(built.entries[0].proxy.ports, ["p1".parse().unwrap()]);
        assert_eq!(
            built.entries[0]
                .source
                .as_ref()
                .map(|s| s.resource.as_str()),
            Some("web-1")
        );
        let messages = built
            .issues
            .iter()
            .map(|issue| issue.message.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            messages,
            [
                "http.app: the proxy is defined more than once",
                "http.app: port not found: missing",
            ]
        );

        let rejected = build(
            DiscoveryProvider::Docker,
            &proxies[..1],
            &ports,
            HashSet::new(),
            |_| Err(Error::InvalidTimeout),
        );
        assert!(rejected.entries.is_empty());
        assert_eq!(rejected.issues.len(), 1);
    }

    #[tokio::test]
    async fn an_unchanged_definition_keeps_its_password_hash() {
        let mut registry = DiscoveryRegistry::default();
        registry.update(DiscoverySnapshot {
            provider: DiscoveryProvider::Docker,
            generation: 0,
            state: DiscoveryState::Running,
            error: None,
            proxies: Some(vec![]),
            issues: vec![],
        });
        let http: HttpProxy = serde_json::from_value(serde_json::json!({
            "routes": [],
            "auth": {"type": "basic", "users": [{"username": "alice", "password": "secret"}]}
        }))
        .unwrap();
        let entry = ProxyEntry::from((
            "abc".parse().unwrap(),
            Proxy {
                kind: ProxyKind::Http(Box::new(http)),
                ..Default::default()
            },
        ));

        let (first, issues) = registry
            .seal(DiscoveryProvider::Docker, vec![entry.clone()])
            .await;
        assert!(issues.is_empty());
        assert_ne!(first[0].proxy, entry.proxy);
        let (second, _) = registry.seal(DiscoveryProvider::Docker, vec![entry]).await;
        assert_eq!(first, second);
    }
}
