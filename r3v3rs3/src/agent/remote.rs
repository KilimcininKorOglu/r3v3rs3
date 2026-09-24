//! The runtime of an agent target: every method is one request on the link of the agent. The
//! published ports of a container become the ports of loopback forwarders on the master, so the
//! pipeline, the health check and the proxy use them as they use the ports of a local container.

use super::forward::{ForwardKey, Forwarders};
use super::link::Link;
use super::protocol::{AgentOutput, AgentRequest};
use super::registry::AgentRegistry;
use crate::runtime::ContainerRuntime;
#[cfg(test)]
use anyhow::Context as _;
use anyhow::anyhow;
use r3v3rs3_api::container::{
    AppName, ContainerInfo, ContainerName, ContainerSpec, ContainerSummary, ImageInfo, ImageRef,
    NetworkName, ProjectName,
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::git::RelPath;
use r3v3rs3_api::id::ShortId;
use std::sync::Arc;
use std::time::Duration;

pub struct RemoteRuntime {
    target: ShortId,
    agents: Arc<AgentRegistry>,
    forwarders: Arc<Forwarders>,
}

impl RemoteRuntime {
    pub fn new(target: ShortId, agents: Arc<AgentRegistry>, forwarders: Arc<Forwarders>) -> Self {
        Self {
            target,
            agents,
            forwarders,
        }
    }

    fn link(&self) -> Result<Link, Error> {
        self.agents
            .link(self.target)
            .ok_or(Error::AgentOffline { id: self.target })
    }

    async fn call(&self, request: AgentRequest) -> anyhow::Result<AgentOutput> {
        self.link()?.request(&request).await
    }

    async fn call_done(&self, request: AgentRequest) -> anyhow::Result<()> {
        match self.call(request).await? {
            AgentOutput::Done => Ok(()),
            other => Err(unexpected(&other)),
        }
    }

    async fn call_id(
        &self,
        request: AgentRequest,
        payload: Option<&[u8]>,
    ) -> anyhow::Result<String> {
        let link = self.link()?;
        let output = match payload {
            Some(payload) => link.request_with_payload(&request, payload).await?,
            None => link.request(&request).await?,
        };
        match output {
            AgentOutput::Id { id } => Ok(id),
            other => Err(unexpected(&other)),
        }
    }

    /// Replaces the published ports of a container with the ports of their forwarders.
    async fn forwarded(&self, mut info: ContainerInfo) -> anyhow::Result<ContainerInfo> {
        let container: ContainerName = info.name.trim_start_matches('/').parse()?;
        for (port, published) in &mut info.published {
            let key = ForwardKey {
                target: self.target,
                container: container.clone(),
                port: *port,
            };
            *published = self.forwarders.port(&key).await?;
        }
        Ok(info)
    }
}

fn unexpected(output: &AgentOutput) -> anyhow::Error {
    anyhow!("the agent gave an unexpected answer: {output:?}")
}

#[async_trait::async_trait]
impl ContainerRuntime for RemoteRuntime {
    async fn version(&self) -> anyhow::Result<String> {
        match self.call(AgentRequest::Version).await? {
            AgentOutput::Version { version } => Ok(version),
            other => Err(unexpected(&other)),
        }
    }

    async fn pull_image(&self, image: &ImageRef) -> anyhow::Result<()> {
        let image = image.clone();
        self.call_done(AgentRequest::PullImage { image }).await
    }

    async fn inspect_image(&self, image: &ImageRef) -> anyhow::Result<Option<ImageInfo>> {
        let image = image.clone();
        match self.call(AgentRequest::InspectImage { image }).await? {
            AgentOutput::Image { image } => Ok(image),
            other => Err(unexpected(&other)),
        }
    }

    async fn build_image(
        &self,
        context: Vec<u8>,
        dockerfile: &RelPath,
        tag: &ImageRef,
    ) -> anyhow::Result<String> {
        let request = AgentRequest::BuildImage {
            dockerfile: dockerfile.clone(),
            tag: tag.clone(),
        };
        self.call_id(request, Some(&context)).await
    }

    async fn remove_image(&self, image: &ImageRef) -> anyhow::Result<()> {
        let image = image.clone();
        self.call_done(AgentRequest::RemoveImage { image }).await
    }

    async fn prune_project_images(&self, project: &ProjectName, all: bool) -> anyhow::Result<()> {
        let project = project.clone();
        self.call_done(AgentRequest::PruneProjectImages { project, all })
            .await
    }

    async fn ensure_network(&self, network: &NetworkName, app: &AppName) -> anyhow::Result<()> {
        let request = AgentRequest::EnsureNetwork {
            network: network.clone(),
            app: app.clone(),
        };
        self.call_done(request).await
    }

    async fn remove_network(&self, network: &NetworkName) -> anyhow::Result<()> {
        let network = network.clone();
        self.call_done(AgentRequest::RemoveNetwork { network })
            .await
    }

    async fn create_container(&self, spec: &ContainerSpec) -> anyhow::Result<String> {
        let spec = spec.clone();
        self.call_id(AgentRequest::CreateContainer { spec }, None)
            .await
    }

    async fn start_container(&self, name: &ContainerName) -> anyhow::Result<()> {
        let name = name.clone();
        self.call_done(AgentRequest::StartContainer { name }).await
    }

    async fn stop_container(&self, name: &ContainerName, timeout: Duration) -> anyhow::Result<()> {
        let request = AgentRequest::StopContainer {
            name: name.clone(),
            timeout_secs: timeout.as_secs(),
        };
        self.call_done(request).await
    }

    async fn remove_container(&self, name: &ContainerName) -> anyhow::Result<()> {
        let request = AgentRequest::RemoveContainer { name: name.clone() };
        self.call_done(request).await?;
        self.forwarders.close_container(self.target, name);
        Ok(())
    }

    async fn inspect_container(
        &self,
        name: &ContainerName,
    ) -> anyhow::Result<Option<ContainerInfo>> {
        let name = name.clone();
        match self.call(AgentRequest::InspectContainer { name }).await? {
            AgentOutput::Container {
                container: Some(info),
            } => Ok(Some(self.forwarded(info).await?)),
            AgentOutput::Container { container: None } => Ok(None),
            other => Err(unexpected(&other)),
        }
    }

    async fn list_containers(&self, app: &AppName) -> anyhow::Result<Vec<ContainerSummary>> {
        let app = app.clone();
        match self.call(AgentRequest::ListContainers { app }).await? {
            AgentOutput::Containers { containers } => Ok(containers),
            other => Err(unexpected(&other)),
        }
    }

    async fn logs(&self, name: &ContainerName, tail: u32) -> anyhow::Result<String> {
        let name = name.clone();
        match self.call(AgentRequest::Logs { name, tail }).await? {
            AgentOutput::Log { log } => Ok(log),
            other => Err(unexpected(&other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testing::TestAgent;
    use r3v3rs3_api::container::APP_LABEL;
    use std::collections::BTreeMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct Setup {
        remote: RemoteRuntime,
        agent: TestAgent,
        forwarders: Arc<Forwarders>,
    }

    fn setup() -> Setup {
        let target: ShortId = "fzn-txd".parse().unwrap();
        let agents = Arc::new(AgentRegistry::default());
        let agent = TestAgent::connect(&agents, target);
        let forwarders = Arc::new(Forwarders::new(agents.clone()));
        Setup {
            remote: RemoteRuntime::new(target, agents, forwarders.clone()),
            agent,
            forwarders,
        }
    }

    fn spec(name: &str, labels: &[(&str, &str)]) -> ContainerSpec {
        ContainerSpec {
            name: name.parse().unwrap(),
            image: "nginx:1.27".parse().unwrap(),
            env: Vec::new(),
            labels: labels
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<BTreeMap<_, _>>(),
            network: None,
            volumes: Vec::new(),
            restart: Default::default(),
            limits: Default::default(),
            publish: Some(80),
        }
    }

    /// A server on 127.0.0.1 that answers the first bytes of every connection with `pong`.
    async fn echo_server() -> anyhow::Result<u16> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 16];
                let _ = stream.read(&mut buf).await;
                let _ = stream.write_all(b"pong").await;
            }
        });
        Ok(port)
    }

    #[tokio::test]
    async fn the_agent_hides_and_keeps_the_containers_outside_the_platform() -> anyhow::Result<()> {
        let setup = setup();
        let db: ContainerName = "db".parse()?;
        setup.agent.runtime.set(&db, true, &[(5432, 5432)]);
        let remote = &setup.remote;
        assert_eq!(remote.inspect_container(&db).await?, None);
        remote.stop_container(&db, Duration::ZERO).await?;
        remote.remove_container(&db).await?;
        assert!(setup.agent.runtime.state().containers["db"].running);
        assert!(remote.logs(&db, 5).await.is_err());
        assert!(remote.start_container(&db).await.is_err());
        let unlabeled = spec("web", &[]);
        assert!(remote.create_container(&unlabeled).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn the_agent_touches_only_the_networks_of_the_platform() -> anyhow::Result<()> {
        let setup = setup();
        let remote = &setup.remote;
        let app: AppName = "shop".parse()?;
        let bridge: NetworkName = "bridge".parse()?;
        assert!(remote.ensure_network(&bridge, &app).await.is_err());
        assert!(remote.remove_network(&bridge).await.is_err());
        let own: NetworkName = "r3v3rs3-shop".parse()?;
        remote.ensure_network(&own, &app).await?;
        remote.remove_network(&own).await?;
        let project: ProjectName = "other".parse()?;
        assert!(remote.prune_project_images(&project, true).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn the_agent_builds_and_removes_only_the_images_of_the_platform() -> anyhow::Result<()> {
        let setup = setup();
        let remote = &setup.remote;
        let nginx: ImageRef = "nginx:1.27".parse()?;
        assert!(remote.remove_image(&nginx).await.is_err());
        let context = b"context".to_vec();
        let dockerfile: RelPath = "Dockerfile".parse()?;
        let refused = remote.build_image(context.clone(), &dockerfile, &nginx);
        assert!(refused.await.is_err());
        let tag: ImageRef = "r3v3rs3/fzn-txd:bcd-fgh".parse()?;
        let id = remote
            .build_image(context.clone(), &dockerfile, &tag)
            .await?;
        assert!(id.starts_with("sha256:"));
        assert_eq!(setup.agent.runtime.state().builds[0].context, context);
        remote.remove_image(&tag).await?;
        assert_eq!(remote.version().await?, "fake");
        Ok(())
    }

    /// Starts the container `web` of the app `shop`, which publishes port 80 on the agent host.
    async fn start_web(setup: &Setup) -> anyhow::Result<ContainerName> {
        let remote = &setup.remote;
        setup.agent.runtime.state().host_port = echo_server().await?;
        remote.pull_image(&"nginx:1.27".parse()?).await?;
        let web = spec("web", &[(APP_LABEL, "shop")]);
        remote.create_container(&web).await?;
        remote.start_container(&web.name).await?;
        Ok(web.name)
    }

    async fn ping(port: u16) -> anyhow::Result<String> {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        stream.write_all(b"ping").await?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await?;
        Ok(answer)
    }

    #[tokio::test]
    async fn a_published_port_is_reached_through_a_forwarder() -> anyhow::Result<()> {
        let setup = setup();
        let web = start_web(&setup).await?;
        let info = setup
            .remote
            .inspect_container(&web)
            .await?
            .context("info")?;
        let port = info.published[&80];
        assert_ne!(port, setup.agent.runtime.state().host_port);
        assert_eq!(ping(port).await?, "pong");

        // The forwarder closes with its container.
        setup.remote.remove_container(&web).await?;
        let key = ForwardKey {
            target: "fzn-txd".parse()?,
            container: web,
            port: 80,
        };
        assert_eq!(setup.forwarders.existing(&key), None);
        Ok(())
    }

    #[tokio::test]
    async fn an_offline_agent_is_an_api_error() -> anyhow::Result<()> {
        let setup = setup();
        setup.agent.stop().await;
        let err = setup.remote.version().await.unwrap_err();
        assert!(matches!(
            err.downcast_ref::<Error>(),
            Some(Error::AgentOffline { .. })
        ));
        Ok(())
    }
}
