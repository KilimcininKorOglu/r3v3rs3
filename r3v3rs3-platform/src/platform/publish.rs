//! Sends the proxies of the running apps to the server.

use super::proxy::{self, HostServer, LiveApp};
use super::store::RunningDeployment;
use super::{Platform, is_local};
use crate::agent::forward::ForwardKey;
use crate::command::ServerCommand;
use crate::discovery::DiscoveredProxy;
use r3v3rs3_api::discovery::DiscoveryIssue;
use r3v3rs3_api::firewall::HostPort;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::AgentLinkQuality;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Weak;
use std::time::Duration;

/// The time between two reads of the running containers. A restart of the Docker Engine can give
/// a container a new published port, and the next read sends it to the server.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(15);

/// The proxies and the issues of one read.
pub type Published = (Vec<DiscoveredProxy>, Vec<DiscoveryIssue>);

/// The host ports that the running apps of each target need open.
pub type TargetPorts = BTreeMap<ShortId, BTreeSet<HostPort>>;

/// Groups the host ports of the running deployments by the targets that run them.
pub fn host_ports_by_target(deployments: &[RunningDeployment]) -> TargetPorts {
    let mut ports = TargetPorts::new();
    for deployment in deployments {
        for target in deployment.spec.targets_or_default(deployment.target) {
            ports
                .entry(target)
                .or_default()
                .extend(deployment.spec.host_ports.iter().copied());
        }
    }
    ports.retain(|_, set| !set.is_empty());
    ports
}

/// The host ports of the local target, which this server's own firewall opens.
pub fn local_ports(ports: &TargetPorts) -> Vec<HostPort> {
    ports
        .iter()
        .filter(|(target, _)| is_local(**target))
        .flat_map(|(_, set)| set.iter().copied())
        .collect()
}

impl Platform {
    /// Reads the running containers and sends their proxies to the server when they changed.
    /// Returns false when the server stopped.
    ///
    /// The server loop must not call it, because it waits for the server loop to take the
    /// snapshot.
    pub async fn publish(&self) -> bool {
        if !self.publish_host_ports().await {
            return false;
        }
        let mut sent = self.sent.lock().await;
        let snapshot = match self.read_proxies().await {
            Ok(current) if sent.as_ref() == Some(&current) => return true,
            Ok(current) => {
                *sent = Some(current.clone());
                proxy::snapshot(current.0, current.1)
            }
            Err(err) => {
                // The next successful read is sent even when it did not change.
                *sent = None;
                proxy::error_snapshot(format!("{err:#}"))
            }
        };
        self.command
            .send(ServerCommand::SetDiscovery { snapshot })
            .await
            .is_ok()
    }

    /// Sends the host ports of the local target to the server when they changed. Returns false
    /// when the server stopped.
    async fn publish_host_ports(&self) -> bool {
        let deployments = match self.store.running_deployments().await {
            Ok(deployments) => deployments,
            Err(err) => {
                tracing::error!(%err, "cannot read the host ports of the running apps");
                return true;
            }
        };
        let ports = host_ports_by_target(&deployments);
        let mut sent = self.sent_host_ports.lock().await;
        if sent.as_ref() == Some(&ports) {
            return true;
        }
        let local = local_ports(&ports);
        *sent = Some(ports);
        self.command
            .send(ServerCommand::SetAppHostPorts { ports: local })
            .await
            .is_ok()
    }

    async fn read_proxies(&self) -> anyhow::Result<Published> {
        let mut proxies = Vec::new();
        let mut issues = Vec::new();
        for deployment in self.store.running_deployments().await? {
            let (host_ports, app_issues) = self.deployment_ports(&deployment).await?;
            issues.extend(app_issues);
            if !host_ports.is_empty() {
                let app = LiveApp {
                    id: deployment.app,
                    name: deployment.app_name.as_str(),
                    spec: &deployment.spec,
                    host_ports,
                };
                proxies.extend(proxy::app_proxy(&app, &self.config)?);
            }
        }
        Ok((proxies, issues))
    }

    async fn deployment_ports(
        &self,
        deployment: &RunningDeployment,
    ) -> anyhow::Result<(Vec<HostServer>, Vec<DiscoveryIssue>)> {
        let mut host_ports = Vec::new();
        let mut issues = Vec::new();
        let targets = self.store.targets().await.unwrap_or_default();
        let drained: std::collections::HashSet<_> =
            targets.iter().filter(|t| t.drained).map(|t| t.id).collect();
        let regions: std::collections::HashMap<_, _> = targets
            .iter()
            .filter_map(|t| t.region.as_ref().map(|r| (t.id, r.clone())))
            .collect();
        for target in deployment.spec.targets_or_default(deployment.target) {
            if drained.contains(&target) {
                continue;
            }
            let weight = self.target_weight(target);
            let region = regions.get(&target).cloned();
            match self.published_port(deployment, target).await? {
                Ok(port) => host_ports.push(HostServer::with_region(port, weight, region)),
                Err(message) => issues.push(DiscoveryIssue {
                    resource: deployment.app_name.to_string(),
                    message,
                }),
            }
        }
        Ok((host_ports, issues))
    }

    fn target_weight(&self, target: ShortId) -> u16 {
        if is_local(target) {
            return AgentLinkQuality::Excellent.default_weight();
        }
        self.agents
            .status(target)
            .map_or(AgentLinkQuality::Offline.default_weight(), |status| {
                status.quality.default_weight()
            })
    }

    /// The port of 127.0.0.1 that reaches the app port of a running deployment, or the reason why
    /// the container has none.
    async fn published_port(
        &self,
        deployment: &RunningDeployment,
        target: ShortId,
    ) -> anyhow::Result<Result<u16, String>> {
        let name = proxy::container_name(deployment.app, deployment.id)?;
        let port = deployment.spec.port;
        let info = match self.runtime(target).inspect_container(&name).await {
            Ok(info) => info,
            // The app of an unreachable agent keeps its route, and its forwarder closes every
            // connection until the agent is back, so the proxy answers 502.
            Err(err) if !is_local(target) => {
                let key = ForwardKey {
                    target,
                    container: name,
                    port,
                };
                return Ok(self.forwarders.existing(&key).ok_or(format!("{err:#}")));
            }
            Err(err) => return Err(err),
        };
        Ok(match info {
            Some(info) if info.running => info
                .port_on_host(port)
                .ok_or_else(|| format!("the container {name} does not publish port {port}")),
            Some(_) => Err(format!("the container {name} is not running")),
            None => Err(format!("the container {name} does not exist")),
        })
    }
}

/// Sends the proxies at the start and after each interval, until the platform or the server stops.
pub async fn refresh(platform: Weak<Platform>) {
    let mut interval = tokio::time::interval(REFRESH_INTERVAL);
    loop {
        let cancel = match platform.upgrade() {
            Some(p) => p.shutdown_token().clone(),
            None => return,
        };
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = interval.tick() => {}
        }
        let Some(platform) = platform.upgrade() else {
            return;
        };
        if !platform.publish().await {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::FakeRuntime;
    use super::super::store::NewDeployment;
    use super::super::tests::{platform_with, request};
    use super::*;
    use crate::discovery::DiscoverySnapshot;
    use r3v3rs3_api::container::ContainerName;
    use r3v3rs3_api::discovery::DiscoveryState;
    use r3v3rs3_api::platform::{DeploymentTrigger, PlatformConfig};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    struct Setup {
        platform: Platform,
        runtime: Arc<FakeRuntime>,
        commands: mpsc::Receiver<ServerCommand>,
        _dir: super::super::tests::TempDir,
    }

    async fn setup() -> anyhow::Result<Setup> {
        let config = PlatformConfig {
            proxy_ports: vec!["http".into()],
            ..Default::default()
        };
        let (command, commands) = mpsc::channel(1);
        let (platform, runtime, dir) = platform_with(&config, command).await?;
        Ok(Setup {
            platform,
            runtime,
            commands,
            _dir: dir,
        })
    }

    /// Adds an app with a domain and a running deployment, and returns its container name.
    async fn deploy(platform: &Platform, name: &str) -> anyhow::Result<ContainerName> {
        let mut app = request(name);
        app.spec.domains = vec![format!("{name}.example.com").parse()?];
        let app = platform.add_app(app, 1).await?;
        let mut deployment =
            NewDeployment::new(&app, DeploymentTrigger::Manual, "alice", &app.spec, &[])?;
        let id = deployment.id;
        deployment.started_at = 2;
        platform.store.insert_deployment(&deployment).await?;
        platform.store.finish_deployment(app.id, id, 3).await?;
        proxy::container_name(app.id, id)
    }

    fn take(commands: &mut mpsc::Receiver<ServerCommand>) -> Option<DiscoverySnapshot> {
        match commands.try_recv() {
            Ok(ServerCommand::SetDiscovery { snapshot }) => Some(snapshot),
            _ => None,
        }
    }

    fn resources(snapshot: &DiscoverySnapshot) -> Vec<String> {
        snapshot
            .proxies
            .iter()
            .flatten()
            .map(|proxy| proxy.source.resource.clone())
            .collect()
    }

    #[tokio::test]
    async fn a_running_container_gets_a_proxy_and_a_missing_one_an_issue() -> anyhow::Result<()> {
        let mut setup = setup().await?;
        let shop = deploy(&setup.platform, "shop").await?;
        let blog = deploy(&setup.platform, "blog").await?;
        setup.runtime.set(&shop, true, &[(80, 49153)]);
        setup.runtime.set(&blog, false, &[]);

        assert!(setup.platform.publish().await);
        let snapshot = take(&mut setup.commands).expect("a snapshot");
        assert_eq!(snapshot.state, DiscoveryState::Running);
        assert_eq!(resources(&snapshot), ["shop"]);
        let proxy = &snapshot.proxies.as_ref().unwrap()[0];
        assert_eq!(proxy.definition.ports, ["http"]);
        assert_eq!(
            crate::discovery::first_route_servers(proxy),
            ["http://127.0.0.1:49153/"]
        );
        let issues = snapshot
            .issues
            .iter()
            .map(|issue| issue.resource.as_str())
            .collect::<Vec<_>>();
        assert_eq!(issues, ["blog"]);

        // An unchanged read sends nothing.
        assert!(setup.platform.publish().await);
        assert!(take(&mut setup.commands).is_none());

        // A new published port is sent.
        setup.runtime.set(&blog, true, &[(80, 49154)]);
        assert!(setup.platform.publish().await);
        let snapshot = take(&mut setup.commands).expect("a snapshot");
        assert_eq!(resources(&snapshot), ["blog", "shop"]);
        assert!(snapshot.issues.is_empty());
        Ok(())
    }

    /// A running app with host ports makes the server open them; an unchanged read sends no
    /// second update, and an app that no longer runs closes them again.
    #[tokio::test]
    async fn the_host_ports_of_running_local_apps_reach_the_server() -> anyhow::Result<()> {
        let mut setup = setup().await?;
        let app = run_turn_server(&setup.platform).await?;

        assert!(setup.platform.publish_host_ports().await);
        assert_eq!(
            sent_ports(&mut setup.commands)?,
            ["udp/3478", "udp/49160-49240"]
        );

        assert!(setup.platform.publish_host_ports().await);
        assert!(setup.commands.try_recv().is_err(), "an unchanged read sent");

        assert!(setup.platform.store.delete_app(app).await?);
        assert!(setup.platform.publish_host_ports().await);
        assert!(sent_ports(&mut setup.commands)?.is_empty());
        Ok(())
    }

    /// Adds an app with two host ports and a running deployment on the local target.
    async fn run_turn_server(platform: &Platform) -> anyhow::Result<ShortId> {
        let mut app = request("turn");
        app.spec.host_ports = vec!["udp/3478".parse()?, "udp/49160-49240".parse()?];
        let app = platform.add_app(app, 1).await?;
        let mut deployment =
            NewDeployment::new(&app, DeploymentTrigger::Manual, "alice", &app.spec, &[])?;
        let id = deployment.id;
        deployment.started_at = 2;
        platform.store.insert_deployment(&deployment).await?;
        platform.store.finish_deployment(app.id, id, 3).await?;
        Ok(app.id)
    }

    /// The host ports of the next command, which must be a host port update.
    fn sent_ports(commands: &mut mpsc::Receiver<ServerCommand>) -> anyhow::Result<Vec<String>> {
        match commands.try_recv() {
            Ok(ServerCommand::SetAppHostPorts { ports }) => {
                Ok(ports.iter().map(ToString::to_string).collect())
            }
            other => anyhow::bail!("expected the host ports, got {other:?}"),
        }
    }

    /// An app on an agent target needs its ports open on the agent host, not on this server.
    #[test]
    fn host_ports_belong_to_the_targets_that_run_the_app() -> anyhow::Result<()> {
        let agent: ShortId = "abc-def".parse()?;
        let mut spec = request("turn").spec;
        spec.host_ports = vec!["udp/3478".parse()?];
        let running = |target| RunningDeployment {
            id: ShortId::default(),
            app: ShortId::default(),
            target,
            app_name: "turn".parse().unwrap(),
            spec: spec.clone(),
        };
        let ports = host_ports_by_target(&[running(agent)]);
        assert_eq!(ports.keys().collect::<Vec<_>>(), [&agent]);
        assert!(local_ports(&ports).is_empty(), "an agent port opened here");
        let local_target: ShortId = r3v3rs3_api::platform::LOCAL_TARGET.parse()?;
        let local = host_ports_by_target(&[running(local_target), running(agent)]);
        assert_eq!(local_ports(&local), [spec.host_ports[0]]);
        let mut plain = running(agent);
        plain.spec.host_ports.clear();
        assert!(host_ports_by_target(&[plain]).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_read_keeps_the_proxies_and_the_next_read_sends_them() -> anyhow::Result<()> {
        let mut setup = setup().await?;
        let shop = deploy(&setup.platform, "shop").await?;
        setup.runtime.set(&shop, true, &[(80, 49153)]);
        assert!(setup.platform.publish().await);
        assert!(take(&mut setup.commands).is_some());

        setup.runtime.state().broken = true;
        assert!(setup.platform.publish().await);
        let snapshot = take(&mut setup.commands).expect("an error snapshot");
        assert_eq!(snapshot.state, DiscoveryState::Error);
        assert!(snapshot.proxies.is_none());

        setup.runtime.state().broken = false;
        assert!(setup.platform.publish().await);
        let snapshot = take(&mut setup.commands).expect("a snapshot");
        assert_eq!(resources(&snapshot), ["shop"]);

        // The server stopped.
        drop(setup.commands);
        setup.runtime.set(&shop, true, &[(80, 49155)]);
        assert!(!setup.platform.publish().await);
        Ok(())
    }
}
