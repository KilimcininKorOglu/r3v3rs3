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
    },
    InspectImage {
        image: ImageRef,
    },
    /// The tar archive of the build context follows the frame as a payload.
    BuildImage {
        dockerfile: RelPath,
        tag: ImageRef,
    },
    RemoveImage {
        image: ImageRef,
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
        spec: ContainerSpec,
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
    Logs {
        name: ContainerName,
        tail: u32,
    },
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
    /// Removes the deployment directories of an app except `keep`.
    ComposeRetain {
        app: ShortId,
        keep: ShortId,
    },
    /// Removes every deployment directory of an app.
    ComposeRemove {
        app: ShortId,
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
    /// The stream is connected to the port.
    Tunnel,
    ComposeModel {
        model: ComposeModel,
    },
}
