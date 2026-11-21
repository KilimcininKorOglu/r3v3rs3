//! The messages of the agent link. The set is closed: the agent never receives a program name,
//! a shell string or raw arguments, and every operand is a type that validates itself.

use crate::build::compose::ComposeModel;
use r3v3rs3_api::container::{
    AppName, ContainerInfo, ContainerName, ContainerSpec, ContainerSummary, EnvVar, ImageInfo,
    ImageRef, NetworkName, ProjectName,
};
use r3v3rs3_api::git::RelPath;
use r3v3rs3_api::id::ShortId;
use serde_derive::{Deserialize, Serialize};
use std::fmt;

/// The only frame of a connection without a client certificate.
#[derive(Clone, Serialize, Deserialize)]
pub struct EnrollRequest {
    /// The secret of the enrollment token.
    pub secret: String,
    /// The certificate signing request of the new agent key in PEM.
    pub csr: String,
    /// The version of the agent.
    pub version: String,
}

// The secret stays out of every log line.
impl fmt::Debug for EnrollRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnrollRequest")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum EnrollResponse {
    Enrolled(Enrolled),
    Refused { message: String },
}

/// The identity of an enrolled agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enrolled {
    pub target: ShortId,
    /// The client certificate of the agent in PEM.
    pub certificate: String,
    /// The CA of the link in PEM.
    pub ca: String,
}

/// A request of the master. Each request travels on its own stream. The requests follow the
/// methods of `ContainerRuntime`. The enums of the link are tagged externally, because an
/// internally tagged enum reads its content through a buffer that cannot read the integer keys of
/// a map, such as the published ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRequest {
    Ping,
    Version,
    PullImage {
        image: ImageRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<r3v3rs3_api::platform::RegistryAuth>,
    },
    InspectImage {
        image: ImageRef,
    },
    /// The tar archive of the build context follows the frame as a payload.
    BuildImage {
        dockerfile: RelPath,
        tag: ImageRef,
    },
    /// A BuildKit build on the agent host; the build context follows as a payload. It is a
    /// variant of its own, because an older agent must refuse it rather than build without
    /// BuildKit.
    BuildImageWithBuildKit {
        dockerfile: RelPath,
        tag: ImageRef,
        buildkit: r3v3rs3_api::platform::BuildKitConfig,
    },
    RemoveImage {
        image: ImageRef,
    },
    /// Keeps the r3v3rs3 firewall table of the agent host: with `enabled` it opens `ports`
    /// and the SSH ports, without it the table is removed.
    SetFirewall {
        enabled: bool,
        ports: Vec<r3v3rs3_api::firewall::HostPort>,
    },
    PruneProjectImages {
        project: ProjectName,
        all: bool,
    },
    EnsureNetwork {
        network: NetworkName,
        app: AppName,
    },
    RemoveNetwork {
        network: NetworkName,
    },
    CreateContainer {
        spec: Box<ContainerSpec>,
    },
    StartContainer {
        name: ContainerName,
    },
    StopContainer {
        name: ContainerName,
        timeout_secs: u64,
    },
    RemoveContainer {
        name: ContainerName,
    },
    InspectContainer {
        name: ContainerName,
    },
    ListContainers {
        app: AppName,
    },
    ListProjectContainers {
        project: ProjectName,
    },
    Logs {
        name: ContainerName,
        tail: u32,
    },
    /// The size of the log files of a container. An older agent refuses the variant.
    ContainerLogInfo {
        name: ContainerName,
    },
    /// Empties the log files of a container. An older agent refuses the variant.
    VacuumContainerLogs {
        name: ContainerName,
    },
    /// Cleans the Docker host of the agent with the switches of the policy, limited to what the
    /// platform owns or can make again. An older agent refuses the variant.
    RunPrune {
        policy: r3v3rs3_api::platform::PrunePolicy,
    },
    /// The usage of the disk that holds the Docker data of the agent host. An older agent
    /// refuses the variant.
    DiskSpace,
    /// Connects the stream to a published port of a container. After the answer the stream
    /// carries the bytes of the connection.
    Tunnel {
        container: ContainerName,
        port: u16,
    },
    /// The tar archive of the deployment directory follows the frame as a payload. The agent
    /// unpacks it and resolves the files of the project.
    ComposeConfig(ComposeRequest),
    /// Starts the services from the directory that `ComposeConfig` unpacked.
    ComposeUp(ComposeRequest),
    ComposeDown {
        project: ProjectName,
    },
    /// Stops the containers of a project and keeps them.
    ComposeStop {
        project: ProjectName,
    },
    /// Starts the stopped containers of a project again.
    ComposeStart {
        project: ProjectName,
    },
    /// Removes the deployment directories of an app except `keep`.
    ComposeRetain {
        app: ShortId,
        keep: ShortId,
    },
    /// Removes every deployment directory of an app.
    ComposeRemove {
        app: ShortId,
    },
    /// Upgrades the agent binary. The new binary follows the frame as a payload.
    Upgrade {
        version: String,
    },
}

/// One deployment of a Compose app. The files are relative to the deployment directory
/// `<app>/<deployment>` in the Compose directory of the agent.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposeRequest {
    pub project: ProjectName,
    pub app: ShortId,
    pub deployment: ShortId,
    pub files: Vec<RelPath>,
    /// The variables that the files interpolate.
    pub env: Vec<EnvVar>,
}

// The values of the variables stay out of every log line.
impl fmt::Debug for ComposeRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComposeRequest")
            .field("project", &self.project)
            .field("app", &self.app)
            .field("deployment", &self.deployment)
            .field("files", &self.files)
            .finish_non_exhaustive()
    }
}

/// The answer of the agent to one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentReply {
    Ok(AgentOutput),
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutput {
    Pong {
        version: String,
    },
    /// The version of the container engine.
    Version {
        version: String,
    },
    Done,
    Image {
        image: Option<ImageInfo>,
    },
    /// The id of a built image or a created container.
    Id {
        id: String,
    },
    Container {
        container: Option<ContainerInfo>,
    },
    Containers {
        containers: Vec<ContainerSummary>,
    },
    Log {
        log: String,
    },
    LogInfo {
        info: Option<r3v3rs3_api::container::ContainerLogInfo>,
    },
    LogVacuum {
        result: r3v3rs3_api::container::LogVacuumResult,
    },
    Prune {
        result: r3v3rs3_api::platform::PruneResult,
    },
    DiskSpace {
        disk: Option<r3v3rs3_api::dashboard::DiskSpace>,
    },
    /// The stream is connected to the port.
    Tunnel,
    ComposeModel {
        model: ComposeModel,
    },
}
