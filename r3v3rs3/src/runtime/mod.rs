//! The container runtimes of the deployment platform. The deploy pipeline talks only to
//! [`ContainerRuntime`], so the same pipeline deploys to the local Docker Engine and, through an
//! agent, to a remote one.

pub mod docker;

use r3v3rs3_api::container::{
    AppName, ContainerInfo, ContainerName, ContainerSpec, ContainerSummary, ImageInfo, ImageRef,
    NetworkName,
};
use r3v3rs3_api::git::RelPath;
use std::time::Duration;

#[async_trait::async_trait]
pub trait ContainerRuntime: Send + Sync {
    /// The version of the container engine.
    async fn version(&self) -> anyhow::Result<String>;

    /// Pulls the image from its registry.
    async fn pull_image(&self, image: &ImageRef) -> anyhow::Result<()>;

    /// Returns `None` when the image is not present.
    async fn inspect_image(&self, image: &ImageRef) -> anyhow::Result<Option<ImageInfo>>;

    /// Builds an image from a tar archive of the build context and tags it with `tag`.
    /// `dockerfile` is relative to the root of the context. Returns the image id.
    async fn build_image(
        &self,
        context: Vec<u8>,
        dockerfile: &RelPath,
        tag: &ImageRef,
    ) -> anyhow::Result<String>;

    /// Removes an image reference. A missing image, and an image that a container still uses,
    /// are not errors.
    async fn remove_image(&self, image: &ImageRef) -> anyhow::Result<()>;

    /// Creates a bridge network unless it exists.
    async fn ensure_network(&self, network: &NetworkName, app: &AppName) -> anyhow::Result<()>;

    /// Removes a network. A missing network is not an error.
    async fn remove_network(&self, network: &NetworkName) -> anyhow::Result<()>;

    /// Creates a container and returns its id.
    async fn create_container(&self, spec: &ContainerSpec) -> anyhow::Result<String>;

    /// Starts a container. A running container is not an error.
    async fn start_container(&self, name: &ContainerName) -> anyhow::Result<()>;

    /// Stops a container, and kills it when it has not stopped after `timeout`. A stopped
    /// container is not an error.
    async fn stop_container(&self, name: &ContainerName, timeout: Duration) -> anyhow::Result<()>;

    /// Removes a container and kills it first when it runs. A missing container is not an error.
    async fn remove_container(&self, name: &ContainerName) -> anyhow::Result<()>;

    /// Returns `None` when the container does not exist.
    async fn inspect_container(
        &self,
        name: &ContainerName,
    ) -> anyhow::Result<Option<ContainerInfo>>;

    /// The containers of an app, running or not.
    async fn list_containers(&self, app: &AppName) -> anyhow::Result<Vec<ContainerSummary>>;

    /// The last `tail` lines of the standard output and the standard error of a container.
    async fn logs(&self, name: &ContainerName, tail: u32) -> anyhow::Result<String>;
}
