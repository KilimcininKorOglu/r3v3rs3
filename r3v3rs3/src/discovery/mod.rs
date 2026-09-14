//! Service discovery. A provider reads proxy definitions from an external system and sends a
//! snapshot to the server. The server turns the definitions into read-only proxies.

pub mod consul;
pub mod docker;
pub mod etcd;
pub mod http;
pub mod ids;
pub mod kubernetes;
pub mod kv;
pub mod labels;
mod tree;

use crate::certs::Cert;
use crate::command::ServerCommand;
use anyhow::anyhow;
use http::ApiClient;
use r3v3rs3_api::discovery::{
    DiscoveryConfig, DiscoveryIssue, DiscoveryProvider, DiscoverySource, DiscoveryState,
};
use r3v3rs3_api::proxy::ProxyKind;
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::warn;

pub const MIN_BACKOFF: Duration = Duration::from_secs(1);
pub const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Changes that arrive within this time cause one read.
pub const DEBOUNCE: Duration = Duration::from_secs(1);

/// A proxy that a label set or a resource defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyDefinition {
    /// `<protocol>.<name>`, which is unique in one resource.
    pub key: String,
    pub name: String,
    /// Port names or port ids.
    pub ports: Vec<String>,
    pub active: bool,
    pub kind: ProxyKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProxy {
    /// Identifies the proxy across reads, so the proxy keeps its id.
    pub key: String,
    pub source: DiscoverySource,
    pub definition: ProxyDefinition,
}

/// The result of one read of a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverySnapshot {
    pub provider: DiscoveryProvider,
    /// The provider task that sent the snapshot. The server ignores a snapshot of a stopped task.
    /// A snapshot of a provider that the server did not start uses 0.
    pub generation: u64,
    pub state: DiscoveryState,
    pub error: Option<String>,
    /// `None` keeps the proxies and the certificates of the previous snapshot, for example after a
    /// connection error.
    pub proxies: Option<Vec<DiscoveredProxy>>,
    /// The server certificates of the read. The server uses them only together with `proxies`.
    pub certs: Vec<Arc<Cert>>,
    pub issues: Vec<DiscoveryIssue>,
}

/// The proxies, the certificates and the issues of one read.
#[derive(Debug, Default)]
pub struct Built {
    pub proxies: Vec<DiscoveredProxy>,
    pub certs: Vec<Arc<Cert>>,
    pub issues: Vec<DiscoveryIssue>,
}

impl Built {
    /// Adds an issue. The same issue of more than one instance is added once.
    pub fn issue(&mut self, resource: &str, message: String) {
        let issue = DiscoveryIssue {
            resource: resource.to_string(),
            message,
        };
        if !self.issues.contains(&issue) {
            self.issues.push(issue);
        }
    }

    /// Adds the definitions and the label issues of a resource.
    pub fn add_parsed(
        &mut self,
        groups: &mut ProxyGroups,
        group: &str,
        resource: &str,
        parsed: labels::Parsed,
    ) {
        for message in parsed.issues {
            self.issue(resource, message);
        }
        for definition in parsed.definitions {
            if let Err(message) = groups.add(group, resource, definition) {
                self.issue(resource, message);
            }
        }
    }
}

/// The proxies of one read by key. The instances of a group add their servers to the same proxy.
#[derive(Debug)]
pub struct ProxyGroups {
    provider: DiscoveryProvider,
    proxies: BTreeMap<String, DiscoveredProxy>,
}

impl ProxyGroups {
    pub fn new(provider: DiscoveryProvider) -> Self {
        Self {
            provider,
            proxies: BTreeMap::new(),
        }
    }

    /// Adds a definition. A definition with the key of an added proxy adds its servers to it.
    pub fn add(
        &mut self,
        group: &str,
        resource: &str,
        definition: ProxyDefinition,
    ) -> Result<(), String> {
        match self.proxies.entry(format!("{group}/{}", definition.key)) {
            Entry::Vacant(entry) => {
                let key = entry.key().clone();
                entry.insert(DiscoveredProxy {
                    key,
                    source: DiscoverySource {
                        provider: self.provider,
                        resource: resource.to_string(),
                    },
                    definition,
                });
            }
            Entry::Occupied(mut entry) => {
                let proxy = entry.get_mut();
                append_servers(&mut proxy.definition.kind, definition.kind)
                    .map_err(|message| format!("{}: {message}", definition.key))?;
                let source = &mut proxy.source.resource;
                if !source.split(", ").any(|name| name == resource) {
                    *source = format!("{source}, {resource}");
                }
            }
        }
        Ok(())
    }

    pub fn into_proxies(self) -> Vec<DiscoveredProxy> {
        self.proxies.into_values().collect()
    }
}

fn append_servers(target: &mut ProxyKind, other: ProxyKind) -> Result<(), &'static str> {
    match (target, other) {
        (ProxyKind::Http(target), ProxyKind::Http(other))
            if target.routes.len() == other.routes.len() =>
        {
            for (route, other) in target.routes.iter_mut().zip(other.routes) {
                route.servers.extend(other.servers);
            }
        }
        (ProxyKind::Tcp(target), ProxyKind::Tcp(other)) => {
            target.upstream_servers.extend(other.upstream_servers);
        }
        (ProxyKind::Udp(target), ProxyKind::Udp(other)) => {
            target.upstream_servers.extend(other.upstream_servers);
        }
        _ => return Err("the instances define the proxy with different routes"),
    }
    Ok(())
}

/// Sends the snapshots of one provider task.
pub struct Reporter {
    provider: DiscoveryProvider,
    generation: u64,
    command: mpsc::Sender<ServerCommand>,
}

impl Reporter {
    pub fn new(
        provider: DiscoveryProvider,
        generation: u64,
        command: mpsc::Sender<ServerCommand>,
    ) -> Self {
        Self {
            provider,
            generation,
            command,
        }
    }

    /// Returns false when the server stopped.
    pub async fn send(
        &self,
        state: DiscoveryState,
        error: Option<String>,
        built: Option<Built>,
    ) -> bool {
        let (proxies, certs, issues) = match built {
            Some(built) => (Some(built.proxies), built.certs, built.issues),
            None => (None, Vec::new(), Vec::new()),
        };
        let snapshot = DiscoverySnapshot {
            provider: self.provider,
            generation: self.generation,
            state,
            error,
            proxies,
            certs,
            issues,
        };
        self.command
            .send(ServerCommand::SetDiscovery { snapshot })
            .await
            .is_ok()
    }

    /// Sends the result of a successful read.
    pub async fn running(&self, built: Built) -> anyhow::Result<()> {
        if self.send(DiscoveryState::Running, None, Some(built)).await {
            Ok(())
        } else {
            Err(anyhow!("the server stopped"))
        }
    }
}

/// A provider that reads its resources and follows their changes.
#[async_trait::async_trait]
pub trait Watch: Send + Sync + 'static {
    fn reporter(&self) -> &Reporter;

    /// Reads the resources and follows their changes until an error occurs. A successful read
    /// resets `backoff`.
    async fn watch(&self, backoff: &mut Duration) -> anyhow::Result<Infallible>;
}

/// Runs the watch of a provider, and starts it again after each error with a growing delay.
pub async fn run(provider: impl Watch) {
    let reporter = provider.reporter();
    if !reporter.send(DiscoveryState::Connecting, None, None).await {
        return;
    }
    let mut backoff = MIN_BACKOFF;
    loop {
        let Err(err) = provider.watch(&mut backoff).await;
        warn!(provider = %reporter.provider, %err, "service discovery failed");
        let error = Some(format!("{err:#}"));
        if !reporter.send(DiscoveryState::Error, error, None).await {
            return;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Starts the task of a provider. A provider that reads an HTTP API needs `client`. The
/// Kubernetes provider builds its own client.
pub fn spawn(
    provider: DiscoveryProvider,
    config: &DiscoveryConfig,
    client: Option<ApiClient>,
    command: mpsc::Sender<ServerCommand>,
    generation: u64,
) -> JoinHandle<()> {
    let reporter = Reporter::new(provider, generation, command);
    match (provider, client) {
        (DiscoveryProvider::Kubernetes, _) => {
            tokio::spawn(run(kubernetes::Provider::new(&config.kubernetes, reporter)))
        }
        (DiscoveryProvider::Docker, Some(client)) => {
            tokio::spawn(run(docker::Provider::new(&config.docker, client, reporter)))
        }
        (DiscoveryProvider::Consul, Some(client)) => {
            tokio::spawn(run(consul::Provider::new(&config.consul, client, reporter)))
        }
        (DiscoveryProvider::Etcd, Some(client)) => {
            tokio::spawn(run(etcd::Provider::new(&config.etcd, client, reporter)))
        }
        (_, None) => tokio::spawn(async move {
            let error = Some("the provider has no API client".to_string());
            reporter.send(DiscoveryState::Error, error, None).await;
        }),
    }
}

/// The server URLs of the first route of an HTTP proxy.
#[cfg(test)]
pub fn first_route_servers(proxy: &DiscoveredProxy) -> Vec<String> {
    let ProxyKind::Http(http) = &proxy.definition.kind else {
        panic!("expected an HTTP proxy");
    };
    http.routes[0]
        .servers
        .iter()
        .map(|server| server.url.to_string())
        .collect()
}
