//! The agent side of the requests: runs them on the Docker Engine of the agent host. The agent
//! checks every request itself, so a master reaches only the containers, the networks and the
//! images of the platform.

use super::client::VERSION;
use super::frame::{read_frame, read_payload, write_frame};
use super::link::LinkStream;
use super::protocol::{AgentOutput, AgentReply, AgentRequest};
use crate::build::MAX_CONTEXT_BYTES;
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, bail};
use r3v3rs3_api::container::{APP_LABEL, ContainerInfo, ContainerName, ContainerSpec, ImageRef};
use r3v3rs3_api::git::RelPath;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tracing::debug;

/// The prefix of the networks and the Compose projects of the platform.
pub const RESOURCE_PREFIX: &str = "r3v3rs3-";

/// The prefix of the repositories of the images that the platform builds.
pub const BUILD_REPOSITORY_PREFIX: &str = "r3v3rs3/";

/// The largest build context payload: the context limit and room for the tar headers.
const MAX_PAYLOAD: u64 = MAX_CONTEXT_BYTES + (64 << 20);

pub struct Executor {
    runtime: Arc<dyn ContainerRuntime>,
}

impl Executor {
    pub fn new(runtime: Arc<dyn ContainerRuntime>) -> Self {
        Self { runtime }
    }

    /// Reads one request from a stream of the master and answers it. A tunnel keeps the stream
    /// until one side closes the connection.
    pub async fn answer(&self, mut stream: LinkStream) -> anyhow::Result<()> {
        let request: AgentRequest = read_frame(&mut stream).await?;
        let result = match request {
            AgentRequest::Tunnel { container, port } => {
                return self.tunnel(stream, &container, port).await;
            }
            AgentRequest::BuildImage { dockerfile, tag } => {
                let context = read_payload(&mut stream, MAX_PAYLOAD).await?;
                self.build_image(context, &dockerfile, &tag).await
            }
            request => self.execute(request).await,
        };
        write_frame(&mut stream, &reply(result)).await
    }

    async fn execute(&self, request: AgentRequest) -> anyhow::Result<AgentOutput> {
        let runtime = &*self.runtime;
        match request {
            AgentRequest::Ping => Ok(AgentOutput::Pong {
                version: VERSION.to_string(),
            }),
            AgentRequest::Version => runtime
                .version()
                .await
                .map(|version| AgentOutput::Version { version }),
            AgentRequest::PullImage { image } => runtime.pull_image(&image).await.map(done),
            AgentRequest::InspectImage { image } => runtime
                .inspect_image(&image)
                .await
                .map(|image| AgentOutput::Image { image }),
            AgentRequest::RemoveImage { image } => self.remove_image(&image).await,
            AgentRequest::PruneProjectImages { project, all } => {
                check_prefix(project.as_str(), RESOURCE_PREFIX)
                    .map(|()| runtime.prune_project_images(&project, all))?
                    .await
                    .map(done)
            }
            AgentRequest::EnsureNetwork { network, app } => {
                check_prefix(network.as_str(), RESOURCE_PREFIX)
                    .map(|()| runtime.ensure_network(&network, &app))?
                    .await
                    .map(done)
            }
            AgentRequest::RemoveNetwork { network } => {
                check_prefix(network.as_str(), RESOURCE_PREFIX)
                    .map(|()| runtime.remove_network(&network))?
                    .await
                    .map(done)
            }
            request => self.container_request(request).await,
        }
    }

    /// The requests about one container, which must carry the app label of the platform.
    async fn container_request(&self, request: AgentRequest) -> anyhow::Result<AgentOutput> {
        let runtime = &*self.runtime;
        match request {
            AgentRequest::CreateContainer { spec } => self.create_container(&spec).await,
            AgentRequest::StartContainer { name } => {
                self.owned(&name).await?;
                runtime.start_container(&name).await.map(done)
            }
            AgentRequest::StopContainer { name, timeout_secs } => {
                let timeout = Duration::from_secs(timeout_secs);
                self.if_owned(&name, runtime.stop_container(&name, timeout))
                    .await
            }
            AgentRequest::RemoveContainer { name } => {
                self.if_owned(&name, runtime.remove_container(&name)).await
            }
            AgentRequest::InspectContainer { name } => self
                .platform_container(&name)
                .await
                .map(|container| AgentOutput::Container { container }),
            AgentRequest::ListContainers { app } => runtime
                .list_containers(&app)
                .await
                .map(|containers| AgentOutput::Containers { containers }),
            AgentRequest::Logs { name, tail } => {
                self.owned(&name).await?;
                runtime
                    .logs(&name, tail)
                    .await
                    .map(|log| AgentOutput::Log { log })
            }
            other => bail!("the agent cannot answer {other:?} here"),
        }
    }

    async fn build_image(
        &self,
        context: Vec<u8>,
        dockerfile: &RelPath,
        tag: &ImageRef,
    ) -> anyhow::Result<AgentOutput> {
        check_prefix(tag.repository(), BUILD_REPOSITORY_PREFIX)?;
        let id = self.runtime.build_image(context, dockerfile, tag).await?;
        Ok(AgentOutput::Id { id })
    }

    /// Removes a built image. The agent keeps every other image, because a container outside the
    /// platform can use it.
    async fn remove_image(&self, image: &ImageRef) -> anyhow::Result<AgentOutput> {
        check_prefix(image.repository(), BUILD_REPOSITORY_PREFIX)?;
        self.runtime.remove_image(image).await.map(done)
    }

    async fn create_container(&self, spec: &ContainerSpec) -> anyhow::Result<AgentOutput> {
        spec.validate()?;
        if !spec.labels.contains_key(APP_LABEL) {
            bail!("the container {} has no {APP_LABEL} label", spec.name);
        }
        let id = self.runtime.create_container(spec).await?;
        Ok(AgentOutput::Id { id })
    }

    /// The state of a container of the platform. A container without the app label is not one,
    /// so the master does not see it.
    async fn platform_container(
        &self,
        name: &ContainerName,
    ) -> anyhow::Result<Option<ContainerInfo>> {
        let info = self.runtime.inspect_container(name).await?;
        Ok(info.filter(|info| info.labels.contains_key(APP_LABEL)))
    }

    async fn owned(&self, name: &ContainerName) -> anyhow::Result<ContainerInfo> {
        self.platform_container(name)
            .await?
            .with_context(|| format!("the platform has no container named {name}"))
    }

    /// Runs `action` when the platform owns the container. A missing container needs no action.
    async fn if_owned(
        &self,
        name: &ContainerName,
        action: impl Future<Output = anyhow::Result<()>>,
    ) -> anyhow::Result<AgentOutput> {
        if self.platform_container(name).await?.is_some() {
            action.await?;
        }
        Ok(AgentOutput::Done)
    }

    /// Connects the stream to the published `port` of a running container of the platform.
    async fn tunnel(
        &self,
        mut stream: LinkStream,
        container: &ContainerName,
        port: u16,
    ) -> anyhow::Result<()> {
        let mut tcp = match self.connect(container, port).await {
            Ok(tcp) => tcp,
            Err(err) => return write_frame(&mut stream, &reply(Err(err))).await,
        };
        write_frame(&mut stream, &AgentReply::Ok(AgentOutput::Tunnel)).await?;
        // A client that resets its connection ends the copy with an error, which is normal.
        if let Err(err) = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await {
            debug!(%container, port, %err, "a tunnel ended with an error");
        }
        Ok(())
    }

    async fn connect(&self, container: &ContainerName, port: u16) -> anyhow::Result<TcpStream> {
        let info = self.owned(container).await?;
        let host_port = info
            .published
            .get(&port)
            .copied()
            .filter(|_| info.running)
            .with_context(|| format!("the container {container} does not serve port {port}"))?;
        Ok(TcpStream::connect(("127.0.0.1", host_port)).await?)
    }
}

fn done(_: ()) -> AgentOutput {
    AgentOutput::Done
}

fn reply(result: anyhow::Result<AgentOutput>) -> AgentReply {
    match result {
        Ok(output) => AgentReply::Ok(output),
        Err(err) => AgentReply::Error {
            message: format!("{err:#}"),
        },
    }
}

fn check_prefix(name: &str, prefix: &str) -> anyhow::Result<()> {
    if !name.starts_with(prefix) {
        bail!("{name} is not a resource of the platform");
    }
    Ok(())
}
