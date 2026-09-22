//! Sends the proxies of the running apps to the server.

use super::Platform;
use super::proxy::{self, LiveApp};
use super::store::RunningDeployment;
use crate::command::ServerCommand;
use crate::discovery::DiscoveredProxy;
use r3v3rs3_api::discovery::DiscoveryIssue;
use std::sync::Weak;
use std::time::Duration;

/// The time between two reads of the running containers. A restart of the Docker Engine can give
/// a container a new published port, and the next read sends it to the server.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(15);

/// The proxies and the issues of one read.
pub type Published = (Vec<DiscoveredProxy>, Vec<DiscoveryIssue>);

impl Platform {
    /// Reads the running containers and sends their proxies to the server when they changed.
    /// Returns false when the server stopped.
    ///
    /// The server loop must not call it, because it waits for the server loop to take the
    /// snapshot.
    pub async fn publish(&self) -> bool {
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

    async fn read_proxies(&self) -> anyhow::Result<Published> {
        let mut proxies = Vec::new();
        let mut issues = Vec::new();
        for deployment in self.store.running_deployments().await? {
            match self.published_port(&deployment).await? {
                Ok(host_port) => {
                    let app = LiveApp {
                        id: deployment.app,
                        name: deployment.app_name.as_str(),
                        spec: &deployment.spec,
                        host_port,
                    };
                    proxies.extend(proxy::app_proxy(&app, &self.config)?);
                }
                Err(message) => issues.push(DiscoveryIssue {
                    resource: deployment.app_name.to_string(),
                    message,
                }),
            }
        }
        Ok((proxies, issues))
    }

    /// The port of 127.0.0.1 that reaches the app port of a running deployment, or the reason why
    /// the container has none.
    async fn published_port(
        &self,
        deployment: &RunningDeployment,
    ) -> anyhow::Result<Result<u16, String>> {
        let name = proxy::container_name(deployment.app, deployment.id)?;
        let port = deployment.spec.port;
        Ok(match self.local.inspect_container(&name).await? {
            Some(info) if info.running => info
                .published
                .get(&port)
                .copied()
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
        interval.tick().await;
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
        let id = super::super::new_id()?;
        let deployment = NewDeployment {
            id,
            app: app.id,
            trigger: DeploymentTrigger::Manual,
            username: "alice",
            spec: &app.spec,
            env: &[],
            image_digest: None,
            started_at: 2,
        };
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
