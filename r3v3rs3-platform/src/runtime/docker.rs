//! The local runtime: the Docker Engine API over a Unix socket or TCP.
//!
//! Docker finds a container or a network by its id, then by its name, then by an id prefix. A name
//! that does not exist can therefore match another object whose id starts with the same hex
//! characters. Every call that changes an object first inspects it by name, checks that the name
//! matches, and then uses the returned id.

use super::ContainerRuntime;
use crate::kv::http::{
    ApiClient, Lines, RESPONSE_TIMEOUT, read_body, read_body_limited, read_json,
};
use anyhow::{Context as _, anyhow, bail};
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::CONTENT_TYPE;
use hyper::{Method, Response, StatusCode};
use r3v3rs3_api::container::{
    APP_LABEL, AppName, COMPOSE_PROJECT_LABEL, ContainerHealth, ContainerInfo, ContainerName,
    ContainerSpec, ContainerStats, ContainerSummary, ImageInfo, ImageRef, NetworkName, ProjectName,
    ResourceLimits,
};
use r3v3rs3_api::git::RelPath;
use r3v3rs3_api::platform::{BuildKitConfig, FileEntry};
use serde::de::DeserializeOwned;
use serde_derive::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::io::Read as _;
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// The time a pull may go without a progress line.
const PULL_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// The time that Docker has to accept a build context and start the build.
const BUILD_START_TIMEOUT: Duration = Duration::from_secs(600);

/// The time a build may go without an output line. A long step prints nothing while it runs.
const BUILD_IDLE_TIMEOUT: Duration = Duration::from_secs(1800);

/// The output lines of a build that a build failure carries.
const BUILD_LOG_TAIL: usize = 20;

/// The host address of a published container port.
const PUBLISH_HOST: &str = "127.0.0.1";

/// The content type of a log body that carries the raw output of a TTY container.
const RAW_STREAM: &str = "application/vnd.docker.raw-stream";

/// The largest directory listing that the archive endpoint may answer.
const MAX_ARCHIVE_SIZE: usize = 128 * 1024 * 1024;
/// The largest file that a download may return.
const MAX_DOWNLOAD_SIZE: usize = 64 * 1024 * 1024;
/// The largest file that an upload accepts.
const MAX_UPLOAD_SIZE: usize = 16 * 1024 * 1024;

/// Percent-encodes a query value.
fn encode_path(value: &str) -> String {
    percent_encoding::percent_encode(value.as_bytes(), percent_encoding::NON_ALPHANUMERIC)
        .to_string()
}

/// The last segment of a path.
fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Splits a path into its parent directory and its last segment.
fn split_parent(path: &str) -> anyhow::Result<(String, String)> {
    let trimmed = path.trim_end_matches('/');
    let (parent, file) = trimmed
        .rsplit_once('/')
        .ok_or_else(|| anyhow!("the path {path} has no parent"))?;
    if file.is_empty() {
        bail!("the path {path} names no file");
    }
    Ok((parent.to_string(), file.to_string()))
}

#[derive(Clone)]
pub struct DockerRuntime {
    client: ApiClient,
    cli: Option<DockerCli>,
}

/// What a BuildKit build needs besides the Engine API: the `docker` binary reaches the same
/// engine, unpacks the context into `work_dir` and keeps its state in `docker_config`.
#[derive(Clone, Debug)]
pub struct DockerCli {
    /// The engine in the format of `DOCKER_HOST`.
    pub docker_host: String,
    /// A writable directory for the unpacked build contexts.
    pub work_dir: std::path::PathBuf,
    /// The config directory of the `docker` CLI, when its default is not writable.
    pub docker_config: Option<std::path::PathBuf>,
}

impl DockerRuntime {
    pub fn new(client: ApiClient) -> Self {
        Self { client, cli: None }
    }

    /// Lets the runtime build with BuildKit through the `docker` binary.
    pub fn with_cli(mut self, cli: DockerCli) -> Self {
        self.cli = Some(cli);
        self
    }

    /// Builds with the classic builder of the Engine API.
    async fn classic_build(
        &self,
        context: Vec<u8>,
        dockerfile: &RelPath,
        tag: &ImageRef,
    ) -> anyhow::Result<String> {
        // Version 1 is the classic builder. BuildKit needs a gRPC session besides this request.
        let path = format!(
            "/build?t={}&dockerfile={}&rm=1&forcerm=1&version=1",
            encode(tag.as_str()),
            encode(dockerfile.as_str())
        );
        let request = self
            .client
            .builder(Method::POST, &path)
            .header(CONTENT_TYPE, "application/x-tar")
            .body(Full::new(Bytes::from(context)))?;
        let response = self
            .client
            .send_with(request, BUILD_START_TIMEOUT, &[])
            .await?;
        let mut lines = Lines::new(response);
        let mut output = BuildOutput::default();
        loop {
            let line = tokio::time::timeout(BUILD_IDLE_TIMEOUT, lines.next())
                .await
                .map_err(|_| anyhow!("the build of {tag} stopped sending output"))??;
            let Some(line) = line else {
                break;
            };
            output
                .read(&line)
                .map_err(|err| anyhow!("failed to build {tag}: {err:#}"))?;
        }
        output
            .id
            .ok_or_else(|| anyhow!("the build of {tag} returned no image id"))
    }

    /// Builds with BuildKit: unpacks the context into its own directory, adds the cache mounts
    /// and runs `docker buildx build`. The directory is removed afterwards.
    async fn buildkit_build(
        &self,
        context: Vec<u8>,
        dockerfile: &RelPath,
        tag: &ImageRef,
        config: &BuildKitConfig,
    ) -> anyhow::Result<String> {
        let cli = self.cli.as_ref().ok_or_else(|| {
            anyhow!("this runtime cannot build with BuildKit, because it has no docker binary")
        })?;
        let dir = cli.work_dir.join(format!(
            "buildkit-{}",
            hex::encode(rand::random::<[u8; 8]>())
        ));
        let result = self
            .buildkit_build_in(cli, &dir, context, dockerfile, tag, config)
            .await;
        if let Err(err) = crate::build::remove_dir(&dir).await {
            tracing::error!(dir = %dir.display(), "failed to remove a build context: {err}");
        }
        result
    }

    async fn buildkit_build_in(
        &self,
        cli: &DockerCli,
        dir: &std::path::Path,
        context: Vec<u8>,
        dockerfile: &RelPath,
        tag: &ImageRef,
        config: &BuildKitConfig,
    ) -> anyhow::Result<String> {
        tokio::fs::create_dir_all(dir).await?;
        let target = dir.to_path_buf();
        tokio::task::spawn_blocking(move || tar::Archive::new(&context[..]).unpack(&target))
            .await??;
        let dockerfile_path = dir.join(dockerfile.as_str());
        crate::build::buildkit::prepare_dockerfile(&dockerfile_path, config).await?;
        let runner = crate::build::buildkit::BuildKitRunner::new(
            Some(cli.docker_host.clone()),
            cli.docker_config.clone(),
        );
        runner.build(dir, &dockerfile_path, tag, config).await?;
        let image = self
            .inspect_image(tag)
            .await?
            .ok_or_else(|| anyhow!("BuildKit built {tag}, but the engine does not have it"))?;
        Ok(image.id)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        allowed: &[StatusCode],
        timeout: Duration,
    ) -> anyhow::Result<Response<Incoming>> {
        let builder = self.client.builder(method, path);
        let request = match body {
            Some(body) => builder
                .header(CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(serde_json::to_vec(body)?)))?,
            None => builder.body(Full::new(Bytes::new()))?,
        };
        self.client.send_with(request, timeout, allowed).await
    }

    /// Sends a request whose body is not needed.
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        allowed: &[StatusCode],
    ) -> anyhow::Result<()> {
        let response = self
            .send(method, path, body, allowed, RESPONSE_TIMEOUT)
            .await?;
        read_body(response).await?;
        Ok(())
    }

    /// Reads a JSON document, or `None` for `404 Not Found`.
    async fn get_optional<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<Option<T>> {
        let allowed = [StatusCode::NOT_FOUND];
        let response = self
            .send(Method::GET, path, None, &allowed, RESPONSE_TIMEOUT)
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            read_body(response).await?;
            return Ok(None);
        }
        Ok(Some(read_json(response).await?))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let response = self
            .send(Method::GET, path, None, &[], RESPONSE_TIMEOUT)
            .await?;
        read_json(response).await
    }

    /// The raw inspect document of the container with exactly this name.
    async fn find_container(
        &self,
        name: &ContainerName,
    ) -> anyhow::Result<Option<InspectContainer>> {
        let path = format!("/containers/{name}/json");
        let found = self.get_optional::<InspectContainer>(&path).await?;
        Ok(found.filter(|container| container.name.trim_start_matches('/') == name.as_str()))
    }

    async fn find_network(&self, name: &NetworkName) -> anyhow::Result<Option<InspectNetwork>> {
        let found = self
            .get_optional::<InspectNetwork>(&format!("/networks/{name}"))
            .await?;
        Ok(found.filter(|network| network.name == name.as_str()))
    }

    /// Connects a created container to further networks before it starts.
    async fn join_networks(&self, id: &str, networks: &[NetworkName]) -> anyhow::Result<()> {
        for network in networks {
            let path = format!("/networks/{network}/connect");
            let body = json!({ "Container": id });
            self.call(Method::POST, &path, Some(&body), &[])
                .await
                .with_context(|| format!("joining the network {network}"))?;
        }
        Ok(())
    }

    /// Removes the stopped containers; with `label`, only the stopped containers that carry it.
    async fn prune_docker_containers(&self, label: Option<&str>) -> anyhow::Result<(u64, u64)> {
        let allowed = [StatusCode::CONFLICT];
        let path = match label {
            Some(label) => {
                let filters = json!({ "label": [label] }).to_string();
                format!("/containers/prune?filters={}", encode(&filters))
            }
            None => "/containers/prune".to_string(),
        };
        let res = self
            .send(Method::POST, &path, None, &allowed, RESPONSE_TIMEOUT)
            .await?;
        let val: Value = read_json(res).await?;
        let count = val
            .get("ContainersDeleted")
            .and_then(|v| v.as_array())
            .map_or(0, |a| a.len() as u64);
        let space = val
            .get("SpaceReclaimed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        Ok((count, space))
    }

    /// Removes the dangling layers and, unless `dangling_only`, every unused image except the
    /// builds of the platform. `prune_builds` keeps the newest builds of each app for a
    /// rollback, and a Docker image prune would delete them, because no container uses them.
    async fn prune_docker_images(&self, dangling_only: bool) -> anyhow::Result<(u64, u64)> {
        let (mut count, mut space) = self.prune_dangling_images().await?;
        if !dangling_only {
            let (removed, freed) = self.remove_unused_images().await?;
            count += removed;
            space += freed;
        }
        Ok((count, space))
    }

    /// Deletes each tagged image that no container uses, except the platform builds. Docker
    /// refuses an image that a container uses with 409, and the image stays.
    async fn remove_unused_images(&self) -> anyhow::Result<(u64, u64)> {
        let images: Vec<ListImage> = self.get("/images/json").await?;
        let (mut count, mut space) = (0, 0);
        for image in images
            .iter()
            .filter(|image| prunable_image(&image.repo_tags))
        {
            let path = format!("/images/{}", image.id);
            let allowed = [StatusCode::NOT_FOUND, StatusCode::CONFLICT];
            let res = self
                .send(Method::DELETE, &path, None, &allowed, RESPONSE_TIMEOUT)
                .await?;
            if res.status().is_success() {
                count += 1;
                space += image.size;
            }
        }
        Ok((count, space))
    }

    async fn prune_dangling_images(&self) -> anyhow::Result<(u64, u64)> {
        let filters = json!({ "dangling": ["true"] }).to_string();
        let path = format!("/images/prune?filters={}", encode(&filters));
        let allowed = [StatusCode::CONFLICT];
        let res = self
            .send(Method::POST, &path, None, &allowed, RESPONSE_TIMEOUT)
            .await?;
        let val: Value = read_json(res).await?;
        let count = val
            .get("ImagesDeleted")
            .and_then(|v| v.as_array())
            .map_or(0, |a| a.len() as u64);
        let space = val
            .get("SpaceReclaimed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        Ok((count, space))
    }

    async fn prune_docker_volumes(&self) -> anyhow::Result<(u64, u64)> {
        let allowed = [StatusCode::CONFLICT];
        let res = self
            .send(
                Method::POST,
                "/volumes/prune",
                None,
                &allowed,
                RESPONSE_TIMEOUT,
            )
            .await?;
        let val: Value = read_json(res).await?;
        let count = val
            .get("VolumesDeleted")
            .and_then(|v| v.as_array())
            .map_or(0, |a| a.len() as u64);
        let space = val
            .get("SpaceReclaimed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        Ok((count, space))
    }

    async fn prune_docker_build_cache(
        &self,
        keep_storage_mb: Option<u64>,
    ) -> anyhow::Result<(u64, u64)> {
        let allowed = [StatusCode::CONFLICT];
        let path = match keep_storage_mb {
            Some(mb) => format!(
                "/build/prune?keep-storage={}",
                mb.saturating_mul(1024 * 1024)
            ),
            None => "/build/prune".to_string(),
        };
        let res = self
            .send(Method::POST, &path, None, &allowed, RESPONSE_TIMEOUT)
            .await?;
        let val: Value = read_json(res).await?;
        let count = val
            .get("CachesDeleted")
            .and_then(|v| v.as_array())
            .map_or(0, |a| a.len() as u64);
        let space = val
            .get("SpaceReclaimed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        Ok((count, space))
    }

    /// Opens an interactive shell in the container with exactly this name. The stream carries
    /// the raw output of a TTY session, so it needs no frame demux.
    pub async fn exec(
        &self,
        name: &ContainerName,
        cols: u16,
        rows: u16,
        cmd: &[&str],
    ) -> anyhow::Result<ExecHandle> {
        let Some(container) = self.find_container(name).await? else {
            bail!("the container {name} is not running");
        };
        let cmd_vec: Vec<&str> = if cmd.is_empty() {
            vec!["/bin/sh"]
        } else {
            cmd.to_vec()
        };
        let response = self
            .send(
                Method::POST,
                &format!("/containers/{}/exec", container.id),
                Some(&json!({
                    "AttachStdin": true,
                    "AttachStdout": true,
                    "AttachStderr": true,
                    "Tty": true,
                    "Cmd": cmd_vec,
                    "Env": ["TERM=xterm-256color"],
                })),
                &[],
                RESPONSE_TIMEOUT,
            )
            .await?;
        let exec: ExecCreateResponse = read_json(response).await?;
        let path = format!("/exec/{}/start", exec.id);
        let builder = self.client.builder(Method::POST, &path);
        let request = builder
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from_static(
                b"{\"Detach\":false,\"Tty\":true}",
            )))?;
        let stream = self.client.send_upgrade(request).await?;
        let handle = ExecHandle {
            stream,
            id: exec.id,
            client: self.client.clone(),
        };
        handle.resize(cols, rows).await?;
        Ok(handle)
    }
}

/// An open exec session of a container.
pub struct ExecHandle {
    /// The duplex stream of the session: stdin and the raw TTY output.
    pub stream: crate::kv::http::UpgradedStream,
    pub client: ApiClient,
    pub id: String,
}

impl ExecHandle {
    /// Resizes the TTY of the open session.
    pub async fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        resize_exec(&self.client, &self.id, cols, rows).await
    }
}

/// Resizes the TTY of an open exec session. A resize of a session that just ended answers
/// `404`, which is fine.
pub async fn resize_exec(client: &ApiClient, id: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
    let path = format!("/exec/{id}/resize?h={rows}&w={cols}");
    let request = client
        .builder(Method::POST, &path)
        .body(Full::new(Bytes::new()))?;
    client
        .send_with(request, RESPONSE_TIMEOUT, &[StatusCode::NOT_FOUND])
        .await?;
    Ok(())
}

impl DockerRuntime {
    /// The volume, build cache and log steps of a prune.
    async fn prune_storage(
        &self,
        policy: &r3v3rs3_api::platform::PrunePolicy,
        result: &mut r3v3rs3_api::platform::PruneResult,
    ) -> anyhow::Result<()> {
        if policy.prune_volumes {
            let (count, space) = self.prune_docker_volumes().await?;
            result.volumes_deleted += count;
            result.space_reclaimed_bytes += space;
        }
        if policy.prune_build_cache {
            let (count, space) = self
                .prune_docker_build_cache(policy.build_cache_keep_storage_mb)
                .await?;
            result.build_cache_deleted += count;
            result.space_reclaimed_bytes += space;
        }
        if policy.prune_logs {
            let threshold_bytes = policy
                .log_vacuum_threshold_mb
                .map(|mb| mb.saturating_mul(1024 * 1024));
            let vacuum_res = self.vacuum_all_logs(threshold_bytes).await?;
            result.logs_truncated += vacuum_res.truncated_files as u64;
            result.space_reclaimed_bytes += vacuum_res.freed_bytes;
        }
        Ok(())
    }

    /// Runs a command in the container and returns its demultiplexed standard output. The
    /// non-TTY exec stream carries stdout and stderr in length-prefixed frames.
    pub async fn exec_capture(
        &self,
        name: &ContainerName,
        command: Vec<String>,
        env: Vec<String>,
    ) -> anyhow::Result<Vec<u8>> {
        let container = self.container_id(name).await?;
        let env: Vec<Value> = env.iter().map(|entry| json!(entry)).collect();
        let command: Vec<Value> = command.into_iter().map(|word| json!(word)).collect();
        let response = self
            .send(
                Method::POST,
                &format!("/containers/{}/exec", container.id),
                Some(&json!({
                    "AttachStdout": true,
                    "AttachStderr": true,
                    "Cmd": command,
                    "Env": env,
                })),
                &[],
                RESPONSE_TIMEOUT,
            )
            .await?;
        let exec: ExecCreateResponse = read_json(response).await?;
        let mut stream = self.start_exec(&exec.id).await?;
        let mut frames = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let read = tokio::time::timeout(RESPONSE_TIMEOUT, stream.read(&mut chunk))
                .await
                .unwrap_or(Ok(0))?;
            if read == 0 {
                break;
            }
            frames.extend_from_slice(&chunk[..read]);
        }
        let state: ExecInspectResponse = self.get(&format!("/exec/{}/json", exec.id)).await?;
        let (stdout, stderr) = split_exec_stream(&frames)?;
        if state.exit_code != 0 {
            bail!(
                "the command {:?} failed with exit code {}: {}{}",
                command,
                state.exit_code,
                String::from_utf8_lossy(&stderr),
                String::from_utf8_lossy(&stdout)
            );
        }
        Ok(stdout)
    }

    /// The resource usage of a container, from one stats document.
    pub async fn stats(&self, name: &ContainerName) -> anyhow::Result<ContainerStats> {
        let container = self.container_id(name).await?;
        let response = self
            .send(
                Method::GET,
                &format!(
                    "/containers/{}/stats?stream=false&one-shot=true",
                    container.id
                ),
                None,
                &[],
                RESPONSE_TIMEOUT,
            )
            .await?;
        let document: StatsDocument = read_json(response).await?;
        Ok(document.into_stats())
    }

    /// The entries of a directory inside a container.
    pub async fn list_dir(
        &self,
        name: &ContainerName,
        path: &str,
    ) -> anyhow::Result<Vec<FileEntry>> {
        let container = self.container_id(name).await?;
        let response = self
            .send(
                Method::GET,
                &format!(
                    "/containers/{}/archive?path={}",
                    container.id,
                    encode_path(path)
                ),
                None,
                &[],
                RESPONSE_TIMEOUT,
            )
            .await?;
        let body = read_body_limited(response, MAX_ARCHIVE_SIZE).await?;
        read_dir_archive(body.to_vec()).await
    }

    /// The content of a file inside a container, up to `MAX_DOWNLOAD_SIZE`.
    pub async fn download_file(&self, name: &ContainerName, path: &str) -> anyhow::Result<Vec<u8>> {
        let container = self.container_id(name).await?;
        let response = self
            .send(
                Method::GET,
                &format!(
                    "/containers/{}/archive?path={}",
                    container.id,
                    encode_path(path)
                ),
                None,
                &[],
                RESPONSE_TIMEOUT,
            )
            .await?;
        let body = read_body_limited(response, MAX_DOWNLOAD_SIZE).await?;
        extract_first_file(body.to_vec()).await
    }

    /// Writes `bytes` into the file at `path` inside a container.
    pub async fn upload_file(
        &self,
        name: &ContainerName,
        path: &str,
        bytes: Vec<u8>,
    ) -> anyhow::Result<()> {
        if bytes.len() > MAX_UPLOAD_SIZE {
            bail!("the file is larger than {MAX_UPLOAD_SIZE} bytes");
        }
        let container = self.container_id(name).await?;
        let (parent, file) = split_parent(path)?;
        let mut archive = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, file, bytes.as_slice())?;
        let body = archive.into_inner()?;
        let path = format!(
            "/containers/{}/archive?path={}",
            container.id,
            encode_path(&parent)
        );
        let request = self
            .client
            .builder(Method::PUT, &path)
            .header(CONTENT_TYPE, "application/x-tar")
            .body(Full::new(Bytes::from(body)))?;
        let response = self
            .client
            .send_with(request, RESPONSE_TIMEOUT, &[])
            .await?;
        read_body(response).await?;
        Ok(())
    }

    /// Removes a file or a directory inside a container with `rm -rf`.
    pub async fn remove_path(&self, name: &ContainerName, path: &str) -> anyhow::Result<()> {
        let container = self.container_id(name).await?;
        let code = self
            .exec_run(&container.id, &["rm", "-rf", "--", path])
            .await?;
        if code != 0 {
            bail!("the container failed to remove {path} with exit code {code}");
        }
        Ok(())
    }

    /// The entire content of a directory inside a container as a tar archive.
    pub async fn download_archive(
        &self,
        name: &ContainerName,
        path: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let container = self.container_id(name).await?;
        let response = self
            .send(
                Method::GET,
                &format!(
                    "/containers/{}/archive?path={}",
                    container.id,
                    encode_path(path)
                ),
                None,
                &[],
                RESPONSE_TIMEOUT,
            )
            .await?;
        let body = read_body_limited(response, MAX_ARCHIVE_SIZE).await?;
        Ok(body.to_vec())
    }

    /// Extracts a tar archive into a directory inside a container.
    pub async fn upload_archive(
        &self,
        name: &ContainerName,
        path: &str,
        archive_tar: Vec<u8>,
    ) -> anyhow::Result<()> {
        let container = self.container_id(name).await?;
        let path = format!(
            "/containers/{}/archive?path={}",
            container.id,
            encode_path(path)
        );
        let request = self
            .client
            .builder(Method::PUT, &path)
            .header(CONTENT_TYPE, "application/x-tar")
            .body(Full::new(Bytes::from(archive_tar)))?;
        let response = self
            .client
            .send_with(request, RESPONSE_TIMEOUT, &[])
            .await?;
        read_body(response).await?;
        Ok(())
    }

    /// The inspect document of the container with exactly this name, or an error.
    async fn container_id(&self, name: &ContainerName) -> anyhow::Result<InspectContainer> {
        self.find_container(name)
            .await?
            .ok_or_else(|| anyhow!("the container {name} is not running"))
    }

    /// Runs a command in the container and returns its exit code.
    async fn exec_run(&self, id: &str, command: &[&str]) -> anyhow::Result<i64> {
        let response = self
            .send(
                Method::POST,
                &format!("/containers/{id}/exec"),
                Some(&json!({
                    "AttachStdout": true,
                    "AttachStderr": true,
                    "Cmd": command,
                })),
                &[],
                RESPONSE_TIMEOUT,
            )
            .await?;
        let exec: ExecCreateResponse = read_json(response).await?;
        let stream = self.start_exec(&exec.id).await?;
        // The output is not shown, but the session has to run to its end.
        let mut leftover = stream;
        let mut chunk = [0u8; 1024];
        while tokio::time::timeout(RESPONSE_TIMEOUT, leftover.read(&mut chunk))
            .await
            .unwrap_or(Ok(0))?
            > 0
        {}
        let state: ExecInspectResponse = self.get(&format!("/exec/{}/json", exec.id)).await?;
        Ok(state.exit_code)
    }

    /// Starts a created exec and returns its duplex stream.
    async fn start_exec(&self, id: &str) -> anyhow::Result<crate::kv::http::UpgradedStream> {
        let path = format!("/exec/{id}/start");
        let request = self
            .client
            .builder(Method::POST, &path)
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from_static(b"{\"Detach\":false}")))?;
        self.client.send_upgrade(request).await
    }
}

/// The entries of the top level of a directory archive.
async fn read_dir_archive(body: Vec<u8>) -> anyhow::Result<Vec<FileEntry>> {
    tokio::task::spawn_blocking(move || {
        let mut entries = Vec::new();
        let mut archive = tar::Archive::new(&body[..]);
        for entry in archive.entries()? {
            let entry = entry?;
            let path = entry.path()?.to_path_buf();
            entries.push(FileEntry {
                name: file_name(&path),
                size: entry.header().entry_size().unwrap_or(0),
                dir: entry.header().entry_type().is_dir(),
            });
        }
        Ok(entries)
    })
    .await?
}

/// The content of the first file entry of an archive.
async fn extract_first_file(body: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        let mut archive = tar::Archive::new(&body[..]);
        for entry in archive.entries()? {
            let mut entry = entry?;
            if entry.header().entry_type().is_file() {
                let mut content = Vec::new();
                entry.read_to_end(&mut content)?;
                return Ok(content);
            }
        }
        anyhow::bail!("the archive holds no file")
    })
    .await?
}

fn registry_auth_header(auth: &r3v3rs3_api::platform::RegistryAuth) -> anyhow::Result<String> {
    use base64::Engine as _;
    let json = serde_json::to_vec(auth)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(json))
}

async fn drain_pull_lines(mut lines: Lines, image: &ImageRef) -> anyhow::Result<()> {
    loop {
        let line = tokio::time::timeout(PULL_IDLE_TIMEOUT, lines.next())
            .await
            .map_err(|_| anyhow!("the pull of {image} stopped sending progress"))??;
        let Some(line) = line else {
            return Ok(());
        };
        pull_error(&line).map_err(|err| anyhow!("failed to pull {image}: {err}"))?;
    }
}

#[async_trait::async_trait]
impl ContainerRuntime for DockerRuntime {
    async fn version(&self) -> anyhow::Result<String> {
        let version: VersionResponse = self.get("/version").await?;
        Ok(version.version)
    }

    async fn pull_image(&self, image: &ImageRef) -> anyhow::Result<()> {
        self.pull_image_with_auth(image, None).await
    }

    async fn pull_image_with_auth(
        &self,
        image: &ImageRef,
        auth: Option<&r3v3rs3_api::platform::RegistryAuth>,
    ) -> anyhow::Result<()> {
        let path = format!(
            "/images/create?fromImage={}&tag={}",
            encode(image.repository()),
            encode(image.tag_or_digest())
        );
        let mut builder = self.client.builder(Method::POST, &path);
        if let Some(auth) = auth {
            let encoded = registry_auth_header(auth)?;
            builder = builder.header("X-Registry-Auth", encoded);
        }
        let request = builder.body(Full::new(Bytes::new()))?;
        let response = self
            .client
            .send_with(request, RESPONSE_TIMEOUT, &[])
            .await?;
        drain_pull_lines(Lines::new(response), image).await
    }

    async fn inspect_image(&self, image: &ImageRef) -> anyhow::Result<Option<ImageInfo>> {
        let found = self
            .get_optional::<InspectImage>(&format!("/images/{image}/json"))
            .await?;
        Ok(found.map(|image| ImageInfo {
            id: image.id,
            repo_digests: image.repo_digests.unwrap_or_default(),
        }))
    }

    async fn build_image(
        &self,
        context: Vec<u8>,
        dockerfile: &RelPath,
        tag: &ImageRef,
        buildkit: Option<&BuildKitConfig>,
    ) -> anyhow::Result<String> {
        match buildkit.filter(|config| config.enabled) {
            Some(config) => self.buildkit_build(context, dockerfile, tag, config).await,
            None => self.classic_build(context, dockerfile, tag).await,
        }
    }

    async fn remove_image(&self, image: &ImageRef) -> anyhow::Result<()> {
        let path = format!("/images/{image}");
        // 409 Conflict: a container still uses the image, so it stays until a later cleanup.
        let allowed = [StatusCode::NOT_FOUND, StatusCode::CONFLICT];
        self.call(Method::DELETE, &path, None, &allowed).await
    }

    async fn prune_project_images(&self, project: &ProjectName, all: bool) -> anyhow::Result<()> {
        let filters = json!({
            "label": [format!("{COMPOSE_PROJECT_LABEL}={project}")],
            "dangling": [(!all).to_string()],
        })
        .to_string();
        let path = format!("/images/prune?filters={}", encode(&filters));
        // 409 Conflict: another prune runs, and the next job prunes again.
        self.call(Method::POST, &path, None, &[StatusCode::CONFLICT])
            .await
    }

    async fn run_prune(
        &self,
        policy: &r3v3rs3_api::platform::PrunePolicy,
    ) -> anyhow::Result<r3v3rs3_api::platform::PruneResult> {
        let mut result = r3v3rs3_api::platform::PruneResult::default();
        if policy.prune_containers {
            let (count, space) = self.prune_docker_containers(None).await?;
            result.containers_deleted += count;
            result.space_reclaimed_bytes += space;
        }
        if policy.prune_images {
            let (count, space) = self
                .prune_docker_images(policy.dangling_layers_only)
                .await?;
            result.images_deleted += count;
            result.space_reclaimed_bytes += space;
        }
        self.prune_storage(policy, &mut result).await?;
        Ok(result)
    }

    async fn prune_platform(
        &self,
        policy: &r3v3rs3_api::platform::PrunePolicy,
    ) -> anyhow::Result<r3v3rs3_api::platform::PruneResult> {
        let mut result = r3v3rs3_api::platform::PruneResult::default();
        if policy.prune_containers {
            let (count, space) = self.prune_docker_containers(Some(APP_LABEL)).await?;
            result.containers_deleted += count;
            result.space_reclaimed_bytes += space;
        }
        if policy.prune_images {
            let (count, space) = self.prune_dangling_images().await?;
            result.images_deleted += count;
            result.space_reclaimed_bytes += space;
        }
        if policy.prune_build_cache {
            let (count, space) = self
                .prune_docker_build_cache(policy.build_cache_keep_storage_mb)
                .await?;
            result.build_cache_deleted += count;
            result.space_reclaimed_bytes += space;
        }
        Ok(result)
    }

    async fn ensure_network(&self, network: &NetworkName, app: &AppName) -> anyhow::Result<()> {
        if self.find_network(network).await?.is_some() {
            return Ok(());
        }
        let body = json!({
            "Name": network.as_str(),
            "Driver": "bridge",
            "Labels": { APP_LABEL: app.as_str() },
        });
        // A concurrent create of the same network answers 409, which leaves the network in place.
        self.call(
            Method::POST,
            "/networks/create",
            Some(&body),
            &[StatusCode::CONFLICT],
        )
        .await?;
        Ok(())
    }

    async fn remove_network(&self, network: &NetworkName) -> anyhow::Result<()> {
        let Some(found) = self.find_network(network).await? else {
            return Ok(());
        };
        let path = format!("/networks/{}", found.id);
        self.call(Method::DELETE, &path, None, &[StatusCode::NOT_FOUND])
            .await?;
        Ok(())
    }

    async fn create_container(&self, spec: &ContainerSpec) -> anyhow::Result<String> {
        spec.validate()?;
        let path = format!("/containers/create?name={}", spec.name);
        let response = self
            .send(
                Method::POST,
                &path,
                Some(&create_body(spec)),
                &[],
                RESPONSE_TIMEOUT,
            )
            .await?;
        let created: CreateResponse = read_json(response).await?;
        if let Err(err) = self.join_networks(&created.id, &spec.extra_networks).await {
            // A container that misses a network cannot reach its database; do not leave it.
            let _ = self.remove_container(&spec.name).await;
            return Err(err);
        }
        Ok(created.id)
    }

    async fn start_container(&self, name: &ContainerName) -> anyhow::Result<()> {
        let id = container_id(self.find_container(name).await?, name)?;
        let path = format!("/containers/{id}/start");
        self.call(Method::POST, &path, None, &[StatusCode::NOT_MODIFIED])
            .await?;
        Ok(())
    }

    async fn stop_container(&self, name: &ContainerName, timeout: Duration) -> anyhow::Result<()> {
        let id = container_id(self.find_container(name).await?, name)?;
        let path = format!("/containers/{id}/stop?t={}", timeout.as_secs());
        // Docker answers after the container has stopped, so wait longer than the stop timeout.
        let response = self
            .send(
                Method::POST,
                &path,
                None,
                &[StatusCode::NOT_MODIFIED],
                timeout + RESPONSE_TIMEOUT,
            )
            .await?;
        read_body(response).await?;
        Ok(())
    }

    async fn remove_container(&self, name: &ContainerName) -> anyhow::Result<()> {
        let Some(found) = self.find_container(name).await? else {
            return Ok(());
        };
        let path = format!("/containers/{}?force=true", found.id);
        self.call(Method::DELETE, &path, None, &[StatusCode::NOT_FOUND])
            .await?;
        Ok(())
    }

    async fn inspect_container(
        &self,
        name: &ContainerName,
    ) -> anyhow::Result<Option<ContainerInfo>> {
        Ok(self
            .find_container(name)
            .await?
            .map(InspectContainer::into_info))
    }

    async fn list_containers(&self, app: &AppName) -> anyhow::Result<Vec<ContainerSummary>> {
        let filters = json!({ "label": [format!("{APP_LABEL}={app}")] }).to_string();
        let path = format!("/containers/json?all=true&filters={}", encode(&filters));
        let list: Vec<ListContainer> = self.get(&path).await?;
        Ok(list.into_iter().map(ListContainer::into_summary).collect())
    }

    async fn update_container_limits(
        &self,
        name: &ContainerName,
        limits: &ResourceLimits,
    ) -> anyhow::Result<()> {
        let Some(found) = self.find_container(name).await? else {
            return Ok(());
        };
        let mut body = json!({});
        if let Some(memory) = limits.memory_bytes {
            body["Memory"] = json!(memory);
        }
        if let Some(cpus) = limits.nano_cpus {
            body["NanoCpus"] = json!(cpus);
        }
        if let Some(swap) = limits.memory_swap_bytes {
            body["MemorySwap"] = json!(swap);
        }
        if let Some(pids) = limits.pids_limit {
            body["PidsLimit"] = json!(pids);
        }
        let path = format!("/containers/{}/update", found.id);
        self.call(Method::POST, &path, Some(&body), &[]).await
    }

    async fn list_project_containers(
        &self,
        project: &ProjectName,
    ) -> anyhow::Result<Vec<ContainerSummary>> {
        let filters =
            json!({ "label": [format!("{COMPOSE_PROJECT_LABEL}={project}")] }).to_string();
        let path = format!("/containers/json?all=true&filters={}", encode(&filters));
        let list: Vec<ListContainer> = self.get(&path).await?;
        Ok(list.into_iter().map(ListContainer::into_summary).collect())
    }

    async fn logs(&self, name: &ContainerName, tail: u32) -> anyhow::Result<String> {
        let id = container_id(self.find_container(name).await?, name)?;
        let path = format!("/containers/{id}/logs?stdout=true&stderr=true&tail={tail}");
        let response = self
            .send(Method::GET, &path, None, &[], RESPONSE_TIMEOUT)
            .await?;
        let raw = response
            .headers()
            .get(CONTENT_TYPE)
            .is_some_and(|value| value == RAW_STREAM);
        let body = read_body(response).await?;
        if raw {
            return Ok(String::from_utf8_lossy(&body).into_owned());
        }
        demux_logs(&body)
    }

    async fn logs_stream(
        &self,
        name: &ContainerName,
        tail: u32,
    ) -> anyhow::Result<(bool, Response<Incoming>)> {
        let id = container_id(self.find_container(name).await?, name)?;
        let path = format!("/containers/{id}/logs?stdout=true&stderr=true&follow=true&tail={tail}");
        let response = self
            .send(Method::GET, &path, None, &[], RESPONSE_TIMEOUT)
            .await?;
        let raw = response
            .headers()
            .get(CONTENT_TYPE)
            .is_some_and(|value| value == RAW_STREAM);
        Ok((raw, response))
    }

    async fn stats(&self, name: &ContainerName) -> anyhow::Result<ContainerStats> {
        DockerRuntime::stats(self, name).await
    }

    async fn exec_capture(
        &self,
        name: &ContainerName,
        command: Vec<String>,
        env: Vec<String>,
    ) -> anyhow::Result<Vec<u8>> {
        DockerRuntime::exec_capture(self, name, command, env).await
    }

    async fn download_file(&self, name: &ContainerName, path: &str) -> anyhow::Result<Vec<u8>> {
        DockerRuntime::download_file(self, name, path).await
    }

    async fn upload_file(
        &self,
        name: &ContainerName,
        path: &str,
        bytes: Vec<u8>,
    ) -> anyhow::Result<()> {
        DockerRuntime::upload_file(self, name, path, bytes).await
    }

    async fn remove_path(&self, name: &ContainerName, path: &str) -> anyhow::Result<()> {
        DockerRuntime::remove_path(self, name, path).await
    }

    async fn download_archive(&self, name: &ContainerName, path: &str) -> anyhow::Result<Vec<u8>> {
        DockerRuntime::download_archive(self, name, path).await
    }

    async fn upload_archive(
        &self,
        name: &ContainerName,
        path: &str,
        archive_tar: Vec<u8>,
    ) -> anyhow::Result<()> {
        DockerRuntime::upload_archive(self, name, path, archive_tar).await
    }

    async fn container_log_info(
        &self,
        name: &ContainerName,
    ) -> anyhow::Result<Option<r3v3rs3_api::container::ContainerLogInfo>> {
        let Some(container) = self.find_container(name).await? else {
            return Ok(None);
        };
        let (size_bytes, rotated_files_count) = match &container.log_path {
            Some(lpath) if !lpath.is_empty() => inspect_container_log_files(lpath)?,
            _ => bail!("the log driver of {name} keeps no log file"),
        };
        Ok(Some(r3v3rs3_api::container::ContainerLogInfo {
            container_id: container.id,
            container_name: container.name.trim_start_matches('/').to_string(),
            log_path: container.log_path,
            size_bytes,
            rotated_files_count,
        }))
    }

    async fn vacuum_container_logs(
        &self,
        name: &ContainerName,
    ) -> anyhow::Result<r3v3rs3_api::container::LogVacuumResult> {
        let Some(container) = self.find_container(name).await? else {
            return Ok(r3v3rs3_api::container::LogVacuumResult::default());
        };
        let (truncated_files, freed_bytes) = match &container.log_path {
            Some(lpath) if !lpath.is_empty() => vacuum_container_log_files(lpath)?,
            _ => bail!("the log driver of {name} keeps no log file"),
        };
        Ok(r3v3rs3_api::container::LogVacuumResult {
            app_id: None,
            truncated_files,
            freed_bytes,
        })
    }

    async fn vacuum_all_logs(
        &self,
        threshold_bytes: Option<u64>,
    ) -> anyhow::Result<r3v3rs3_api::container::LogVacuumResult> {
        let list: Vec<ListContainer> = self.get("/containers/json?all=true").await?;
        let min_bytes = threshold_bytes.unwrap_or(0);
        let mut total_files = 0usize;
        let mut total_freed = 0u64;

        for item in list {
            let path = format!("/containers/{}/json", item.id);
            // A container that went away since the listing has no logs left to vacuum.
            let Some(inspected) = self.get_optional::<InspectContainer>(&path).await? else {
                continue;
            };
            // Another log driver, such as journald, keeps no file that the vacuum could empty.
            let Some(lpath) = inspected.log_path.filter(|path| !path.is_empty()) else {
                continue;
            };
            let (size, _) = inspect_container_log_files(&lpath)?;
            if size >= min_bytes {
                let (files, freed) = vacuum_container_log_files(&lpath)?;
                total_files += files;
                total_freed += freed;
            }
        }

        Ok(r3v3rs3_api::container::LogVacuumResult {
            app_id: None,
            truncated_files: total_files,
            freed_bytes: total_freed,
        })
    }
}

/// The size of the active log file. A container that has written nothing yet has none.
fn active_log_size(path: &std::path::Path) -> anyhow::Result<Option<u64>> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(Some(meta.len())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// The rotated files of a json-file log with their sizes: the files next to it whose name
/// starts with its name.
fn rotated_logs(path: &std::path::Path) -> anyhow::Result<Vec<(std::path::PathBuf, u64)>> {
    let parent = path.parent().context("a log path has no directory")?;
    let name = path.file_name().context("a log path has no file name")?;
    let entries = std::fs::read_dir(parent)
        .with_context(|| format!("failed to list {}", parent.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        if let Some(file) = rotated_log(&entry?, path, name)? {
            files.push(file);
        }
    }
    Ok(files)
}

/// The path and size of a directory entry that is a rotated file of the log `path`.
fn rotated_log(
    entry: &std::fs::DirEntry,
    path: &std::path::Path,
    name: &std::ffi::OsStr,
) -> anyhow::Result<Option<(std::path::PathBuf, u64)>> {
    let file = entry.path();
    let rotated = file != path
        && entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(name.as_encoded_bytes());
    if !rotated || !entry.file_type()?.is_file() {
        return Ok(None);
    }
    Ok(Some((file, entry.metadata()?.len())))
}

/// The total size of the log files of a container and the number of rotated files.
fn inspect_container_log_files(log_path: &str) -> anyhow::Result<(u64, usize)> {
    let path = std::path::Path::new(log_path);
    let active = active_log_size(path)?.unwrap_or(0);
    let rotated = rotated_logs(path)?;
    let size = rotated.iter().map(|(_, size)| size).sum::<u64>() + active;
    Ok((size, rotated.len()))
}

/// Deletes the rotated log files of a container: the files it deleted and the bytes it freed.
fn vacuum_container_log_files(log_path: &str) -> anyhow::Result<(usize, u64)> {
    let path = std::path::Path::new(log_path);
    // The active file stays: Docker Engine keeps its size in memory, and a file emptied from
    // outside stalls `docker logs --tail` until the container restarts. Its size is capped by
    // the `max-size` of the container, and Docker rotates it into a file the next vacuum
    // deletes.
    let files = rotated_logs(path)?;
    for (file, _) in &files {
        std::fs::remove_file(file)
            .with_context(|| format!("failed to delete {}", file.display()))?;
    }
    Ok((files.len(), files.iter().map(|(_, size)| size).sum()))
}

fn container_id(found: Option<InspectContainer>, name: &ContainerName) -> anyhow::Result<String> {
    found
        .map(|container| container.id)
        .ok_or_else(|| anyhow!("no container is named {name}"))
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// The error of one line of a pull progress stream. Docker reports a failed pull inside a
/// `200 OK` stream, so every line is checked.
fn pull_error(line: &[u8]) -> anyhow::Result<()> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let progress: PullProgress = serde_json::from_slice(line)?;
    match progress.error {
        Some(error) => bail!("{error}"),
        None => Ok(()),
    }
}

/// The image id and the last output lines of a build stream.
#[derive(Default)]
struct BuildOutput {
    id: Option<String>,
    tail: VecDeque<String>,
}

impl BuildOutput {
    /// Reads one line of the stream. Docker reports a failed build inside a `200 OK` stream, and
    /// the error then carries the last output lines.
    fn read(&mut self, line: &[u8]) -> anyhow::Result<()> {
        if line.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let progress: BuildProgress = serde_json::from_slice(line)?;
        for text in progress.stream.iter().flat_map(|stream| stream.lines()) {
            self.push(text.trim_end());
        }
        if let Some(error) = progress.error {
            let tail = Vec::from(std::mem::take(&mut self.tail)).join("\n");
            bail!("{error}\n{tail}");
        }
        if let Some(id) = progress.aux.and_then(|aux| aux.id) {
            self.id = Some(id);
        }
        Ok(())
    }

    fn push(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.tail.len() == BUILD_LOG_TAIL {
            self.tail.pop_front();
        }
        self.tail.push_back(text.to_string());
    }
}

/// The body of `POST /containers/create`.
fn create_body(spec: &ContainerSpec) -> Value {
    let env = spec
        .env
        .iter()
        .map(|var| format!("{}={}", var.key.as_str(), var.value))
        .collect::<Vec<_>>();
    let mounts = spec
        .volumes
        .iter()
        .map(|mount| {
            json!({
                "Type": "volume",
                "Source": mount.volume.as_str(),
                "Target": mount.target.as_str(),
                "ReadOnly": mount.read_only,
            })
        })
        .collect::<Vec<_>>();
    let max_size = spec.log_max_size.as_deref().unwrap_or("50m");
    let max_file = spec.log_max_file.as_deref().unwrap_or("3");
    let log_config = json!({
        "Type": "json-file",
        "Config": {
            "max-size": max_size,
            "max-file": max_file,
        }
    });
    let mut host_config = json!({
        "RestartPolicy": { "Name": spec.restart.docker_name() },
        "Mounts": mounts,
        "LogConfig": log_config,
        "SecurityOpt": ["no-new-privileges:true"],
        "CapDrop": ["NET_RAW", "MKNOD", "AUDIT_WRITE"],
    });
    if let Some(memory) = spec.limits.memory_bytes {
        host_config["Memory"] = json!(memory);
    }
    if let Some(cpus) = spec.limits.nano_cpus {
        host_config["NanoCpus"] = json!(cpus);
    }
    if let Some(swap) = spec.limits.memory_swap_bytes {
        host_config["MemorySwap"] = json!(swap);
    }
    if let Some(pids) = spec.limits.pids_limit {
        host_config["PidsLimit"] = json!(pids);
    }
    let mut body = json!({
        "Image": spec.image.as_str(),
        "Env": env,
        "Labels": spec.labels,
        "HostConfig": host_config,
    });
    if let Some(network) = &spec.network {
        let endpoint = if spec.aliases.is_empty() {
            json!({})
        } else {
            json!({ "Aliases": spec.aliases })
        };
        body["HostConfig"]["NetworkMode"] = json!(network.as_str());
        body["NetworkingConfig"] = json!({ "EndpointsConfig": { network.as_str(): endpoint } });
    }
    if let Some(cmd) = &spec.command {
        body["Cmd"] = json!(cmd);
    }
    if let Some(port) = spec.publish {
        let key = format!("{port}/tcp");
        body["ExposedPorts"] = json!({ key.as_str(): {} });
        // A fixed host port keeps the address across deployments; an empty one lets
        // Docker pick a free port.
        let host_port = spec
            .host_port
            .map(|port| port.to_string())
            .unwrap_or_default();
        body["HostConfig"]["PortBindings"] =
            json!({ key.as_str(): [{ "HostIp": PUBLISH_HOST, "HostPort": host_port }] });
    }
    body
}

/// The TCP ports that Docker published on `127.0.0.1`, by container port.
fn published_ports(
    ports: Option<BTreeMap<String, Option<Vec<PortBinding>>>>,
) -> BTreeMap<u16, u16> {
    ports
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, bindings)| {
            let port = key.strip_suffix("/tcp")?.parse().ok()?;
            let host_port = bindings?
                .into_iter()
                .find(|binding| binding.host_ip == PUBLISH_HOST)?
                .host_port
                .parse()
                .ok()?;
            Some((port, host_port))
        })
        .collect()
}

/// Joins the frames of a multiplexed log stream. Each frame has an 8-byte header: the stream type,
/// three zero bytes and the payload size as a big-endian `u32`.
fn demux_logs(data: &[u8]) -> anyhow::Result<String> {
    Ok(String::from_utf8_lossy(&demux_stream(data)?).into_owned())
}

/// Joins the payloads of a multiplexed log stream, stdout and stderr in their order.
fn demux_stream(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut output = Vec::with_capacity(data.len());
    for (_, payload) in stream_frames(data) {
        output.extend_from_slice(payload?);
    }
    Ok(output)
}

/// Splits a multiplexed exec stream into its standard output and its standard error. A
/// command whose output is data, such as a database dump, must not get the warnings and the
/// progress lines that the tool writes to stderr mixed into that data.
fn split_exec_stream(data: &[u8]) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let (mut stdout, mut stderr) = (Vec::with_capacity(data.len()), Vec::new());
    for (stream, payload) in stream_frames(data) {
        match stream {
            STDERR_STREAM => stderr.extend_from_slice(payload?),
            _ => stdout.extend_from_slice(payload?),
        }
    }
    Ok((stdout, stderr))
}

/// The stream type of a frame of the standard error.
const STDERR_STREAM: u8 = 2;

/// The frames of a multiplexed exec or log stream: an 8-byte header per frame (the stream
/// type, three zero bytes and the payload size as a big-endian `u32`), payload after it.
fn stream_frames(mut data: &[u8]) -> impl Iterator<Item = (u8, anyhow::Result<&[u8]>)> {
    std::iter::from_fn(move || {
        if data.is_empty() {
            return None;
        }
        let Some(header) = data.get(..8) else {
            data = &[];
            return Some((0, Err(anyhow!("a log frame header is truncated"))));
        };
        let stream = header[0];
        let size = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let Some(payload) = data.get(8..8 + size) else {
            data = &[];
            return Some((stream, Err(anyhow!("a log frame is truncated"))));
        };
        data = &data[8 + size..];
        Some((stream, Ok(payload)))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VersionResponse {
    version: String,
}

#[derive(Deserialize)]
struct PullProgress {
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct BuildProgress {
    #[serde(default)]
    stream: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    aux: Option<BuildAux>,
}

#[derive(Deserialize)]
struct BuildAux {
    #[serde(rename = "ID", default)]
    id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectImage {
    id: String,
    #[serde(default)]
    repo_digests: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectNetwork {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateResponse {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectContainer {
    id: String,
    name: String,
    image: String,
    state: InspectState,
    #[serde(default)]
    log_path: Option<String>,
    #[serde(default)]
    config: Option<InspectConfig>,
    #[serde(default)]
    network_settings: Option<InspectNetworkSettings>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ExecCreateResponse {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ExecInspectResponse {
    exit_code: i64,
}

/// One stats document of the Docker Engine API, with `stream=false`.
#[derive(Deserialize)]
struct StatsDocument {
    cpu_stats: StatsCpu,
    precpu_stats: StatsCpu,
    #[serde(default)]
    memory_stats: Option<StatsMemory>,
    #[serde(default)]
    networks: Option<BTreeMap<String, StatsNetwork>>,
}

#[derive(Default, Deserialize)]
struct StatsThrottling {
    #[serde(default)]
    periods: u64,
    #[serde(default)]
    throttled_periods: u64,
    #[serde(default)]
    throttled_time: u64,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
struct StatsCpu {
    #[serde(default)]
    cpu_usage: Option<StatsCpuUsage>,
    #[serde(default)]
    system_cpu_usage: Option<u64>,
    #[serde(default)]
    online_cpus: Option<f64>,
    #[serde(default)]
    throttling_data: Option<StatsThrottling>,
}

#[derive(Default, Deserialize)]
struct StatsCpuUsage {
    #[serde(default)]
    total_usage: u64,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
struct StatsMemory {
    #[serde(default)]
    usage: u64,
    #[serde(default)]
    limit: u64,
    #[serde(default)]
    failcnt: Option<u64>,
    #[serde(default)]
    stats: Option<BTreeMap<String, u64>>,
}

#[derive(Default, Deserialize)]
struct StatsNetwork {
    #[serde(default)]
    rx_bytes: u64,
    #[serde(default)]
    tx_bytes: u64,
}

fn compute_cpu_throttling(cpu: &StatsCpu, precpu: &StatsCpu) -> (u64, u64, u64, f64) {
    let curr = cpu.throttling_data.as_ref();
    let prev = precpu.throttling_data.as_ref();
    let total_periods = curr.map(|t| t.periods).unwrap_or(0);
    let throttled_periods = curr.map(|t| t.throttled_periods).unwrap_or(0);
    let throttled_time_ns = curr.map(|t| t.throttled_time).unwrap_or(0);
    let throttled_time_ms = throttled_time_ns / 1_000_000;

    let periods_delta = total_periods.saturating_sub(prev.map(|t| t.periods).unwrap_or(0));
    let throttled_delta =
        throttled_periods.saturating_sub(prev.map(|t| t.throttled_periods).unwrap_or(0));

    let throttled_percent = if periods_delta > 0 {
        (throttled_delta as f64 / periods_delta as f64) * 100.0
    } else if total_periods > 0 {
        (throttled_periods as f64 / total_periods as f64) * 100.0
    } else {
        0.0
    };

    (
        throttled_periods,
        total_periods,
        throttled_time_ms,
        throttled_percent.clamp(0.0, 100.0),
    )
}

fn compute_memory_fail_count(mem: &StatsMemory) -> u64 {
    if let Some(cnt) = mem.failcnt.filter(|&c| c > 0) {
        return cnt;
    }
    if let Some(ref stats) = mem.stats {
        for key in ["failcnt", "oom_kills", "oom"] {
            if let Some(&val) = stats.get(key).filter(|&&v| v > 0) {
                return val;
            }
        }
    }
    0
}

impl StatsDocument {
    /// The CPU percent of the interval between the two samples, the memory, and the summed
    /// network counters of every interface.
    fn into_stats(self) -> ContainerStats {
        let online = self.cpu_stats.online_cpus.unwrap_or(1.0).max(1.0);
        let total = self
            .cpu_stats
            .cpu_usage
            .as_ref()
            .map(|usage| usage.total_usage)
            .unwrap_or(0);
        let previous = self
            .precpu_stats
            .cpu_usage
            .as_ref()
            .map(|usage| usage.total_usage)
            .unwrap_or(0);
        let system = self.cpu_stats.system_cpu_usage.unwrap_or(0);
        let previous_system = self.precpu_stats.system_cpu_usage.unwrap_or(0);
        let cpu_percent = if system > previous_system {
            (total - previous) as f64 / (system - previous_system) as f64 * online * 100.0
        } else {
            0.0
        };
        let memory = self.memory_stats.unwrap_or_default();
        let networks = self.networks.unwrap_or_default();
        let (rx, tx) = networks.into_values().fold((0, 0), |(rx, tx), net| {
            (rx + net.rx_bytes, tx + net.tx_bytes)
        });
        let (throttled_periods, total_periods, throttled_time_ms, throttled_percent) =
            compute_cpu_throttling(&self.cpu_stats, &self.precpu_stats);
        let memory_fail_count = compute_memory_fail_count(&memory);

        ContainerStats {
            cpu_percent,
            memory_bytes: memory.usage,
            memory_limit: memory.limit,
            net_rx_bytes: rx,
            net_tx_bytes: tx,
            cpu_throttled_periods: throttled_periods,
            cpu_total_periods: total_periods,
            cpu_throttled_time_ms: throttled_time_ms,
            cpu_throttled_percent: throttled_percent,
            memory_fail_count,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectState {
    running: bool,
    exit_code: i64,
    #[serde(rename = "OOMKilled", default)]
    oom_killed: bool,
    #[serde(default)]
    health: Option<InspectHealth>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectHealth {
    status: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectConfig {
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectNetworkSettings {
    #[serde(default)]
    networks: Option<BTreeMap<String, InspectEndpoint>>,
    #[serde(default)]
    ports: Option<BTreeMap<String, Option<Vec<PortBinding>>>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PortBinding {
    #[serde(default)]
    host_ip: String,
    #[serde(default)]
    host_port: String,
}

#[derive(Deserialize)]
struct InspectEndpoint {
    #[serde(rename = "IPAddress", default)]
    ip_address: String,
    #[serde(rename = "GlobalIPv6Address", default)]
    global_ipv6_address: String,
}

impl InspectEndpoint {
    fn address(&self) -> Option<&str> {
        [&self.ip_address, &self.global_ipv6_address]
            .into_iter()
            .find(|ip| !ip.is_empty())
            .map(String::as_str)
    }
}

impl InspectContainer {
    fn into_info(self) -> ContainerInfo {
        let settings = self.network_settings.unwrap_or_default();
        let published = published_ports(settings.ports);
        let addresses = settings
            .networks
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(network, endpoint)| {
                endpoint
                    .address()
                    .map(|ip| (network.clone(), ip.to_string()))
            })
            .collect();
        ContainerInfo {
            id: self.id,
            name: self.name.trim_start_matches('/').to_string(),
            image_id: self.image,
            running: self.state.running,
            exit_code: self.state.exit_code,
            oom_killed: self.state.oom_killed,
            health: self
                .state
                .health
                .and_then(|health| health_of(&health.status)),
            addresses,
            labels: self
                .config
                .and_then(|config| config.labels)
                .unwrap_or_default(),
            published,
        }
    }
}

fn health_of(status: &str) -> Option<ContainerHealth> {
    match status {
        "starting" => Some(ContainerHealth::Starting),
        "healthy" => Some(ContainerHealth::Healthy),
        "unhealthy" => Some(ContainerHealth::Unhealthy),
        _ => None,
    }
}

/// One image of `GET /images/json`.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ListImage {
    id: String,
    #[serde(default)]
    repo_tags: Option<Vec<String>>,
    #[serde(default)]
    size: u64,
}

/// Whether a full image prune may delete an image with these tags: a build of the platform
/// stays, because `prune_builds` keeps it for a rollback.
fn prunable_image(tags: &Option<Vec<String>>) -> bool {
    let prefix = crate::agent::executor::BUILD_REPOSITORY_PREFIX;
    !tags.iter().flatten().any(|tag| tag.starts_with(prefix))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ListContainer {
    id: String,
    #[serde(default)]
    names: Option<Vec<String>>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
}

impl ListContainer {
    fn into_summary(self) -> ContainerSummary {
        let name = self
            .names
            .iter()
            .flatten()
            .next()
            .map(|name| name.trim_start_matches('/').to_string())
            .unwrap_or_default();
        ContainerSummary {
            id: self.id,
            name,
            state: self.state,
            labels: self.labels.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::container::{EnvVar, ResourceLimits, RestartPolicy, VolumeMount};

    fn frame(stream: u8, payload: &str) -> Vec<u8> {
        let mut frame = vec![stream, 0, 0, 0];
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload.as_bytes());
        frame
    }

    /// A rollback starts the build image of an older deployment, which no container uses, so
    /// a full image prune must keep every image of the `r3v3rs3/` builds and delete the others.
    #[test]
    fn a_full_image_prune_keeps_the_builds_of_the_platform() {
        let tags = |tags: &[&str]| Some(tags.iter().map(|tag| tag.to_string()).collect());
        assert!(!prunable_image(&tags(&["r3v3rs3/shop:abc-def"])));
        assert!(!prunable_image(&tags(&[
            "nginx:1.27",
            "r3v3rs3/shop:abc-def"
        ])));
        assert!(prunable_image(&tags(&["nginx:1.27"])));
        assert!(prunable_image(&tags(&["r3v3rs3-shop-web:latest"])));
        assert!(prunable_image(&None));
    }

    #[test]
    fn log_frames_are_joined_in_order() {
        let mut data = frame(1, "out 1\n");
        data.extend(frame(2, "err 1\n"));
        data.extend(frame(1, "out 2\n"));
        assert_eq!(demux_logs(&data).unwrap(), "out 1\nerr 1\nout 2\n");
        assert_eq!(demux_logs(&[]).unwrap(), "");
    }

    #[test]
    fn a_truncated_log_frame_is_an_error() {
        let data = frame(1, "hello");
        assert!(demux_logs(&data[..6]).is_err());
        assert!(demux_logs(&data[..10]).is_err());
    }

    #[test]
    fn a_stats_document_becomes_container_stats() {
        let document = json!({
            "cpu_stats": {
                "cpu_usage": { "total_usage": 2_000_000u64 },
                "system_cpu_usage": 10_000_000_000u64,
                "online_cpus": 2u64,
            },
            "precpu_stats": {
                "cpu_usage": { "total_usage": 1_000_000u64 },
                "system_cpu_usage": 9_000_000_000u64,
            },
            "memory_stats": { "usage": 5_000_000u64, "limit": 1_000_000_000u64 },
            "networks": {
                "eth0": { "rx_bytes": 100u64, "tx_bytes": 50u64 },
                "lo": { "rx_bytes": 10u64, "tx_bytes": 10u64 },
            },
        });
        let stats = serde_json::from_value::<StatsDocument>(document)
            .unwrap()
            .into_stats();
        // 1 ms of one core over 1 s of two cores is 0.2 %.
        assert!(
            (stats.cpu_percent - 0.2).abs() < 1e-9,
            "{:?}",
            stats.cpu_percent
        );
        assert_eq!(stats.memory_bytes, 5_000_000);
        assert_eq!(stats.memory_limit, 1_000_000_000);
        assert_eq!(stats.net_rx_bytes, 110);
        assert_eq!(stats.net_tx_bytes, 60);
    }

    #[test]
    fn a_stats_document_without_samples_is_zeroed() {
        let stats = serde_json::from_value::<StatsDocument>(json!({
            "cpu_stats": {}, "precpu_stats": {},
        }))
        .unwrap()
        .into_stats();
        assert_eq!(stats, ContainerStats::default());
    }

    /// The json-file log driver writes `<id>-json.log` and rotates it into `<id>-json.log.N`
    /// next to it. The size counts both. The vacuum deletes the rotated files and leaves the
    /// active file intact: Docker Engine keeps the size of the active file in memory, so
    /// emptying it from outside stalls `docker logs --tail` until the container restarts
    /// (measured on Docker 29). A log that cannot be read is an error, never a size of 0.
    #[test]
    fn container_logs_are_measured_and_emptied_or_the_error_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("r3v3rs3-logs-{:08x}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("abc-json.log");
        std::fs::write(&log, b"0123456789").unwrap();
        std::fs::write(dir.join("abc-json.log.1"), b"01234").unwrap();
        std::fs::write(dir.join("other-json.log"), b"0123456789012").unwrap();
        let path = log.to_str().unwrap();
        assert_eq!(inspect_container_log_files(path).unwrap(), (15, 1));
        assert_eq!(vacuum_container_log_files(path).unwrap(), (1, 5));
        assert_eq!(std::fs::read(&log).unwrap(), b"0123456789");
        assert!(!dir.join("abc-json.log.1").exists());
        assert_eq!(
            std::fs::metadata(dir.join("other-json.log")).unwrap().len(),
            13
        );
        // The server user cannot list the directory of the Docker logs.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o100)).unwrap();
        let unreadable = inspect_container_log_files(path);
        let unwritable = vacuum_container_log_files(path);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(unreadable.is_err());
        assert!(unwritable.is_err());
    }

    /// The Docker exec stream protocol tags each frame with its stream: 1 is stdout, 2 is
    /// stderr. A log shows both; the output of a command, such as a database dump, is its
    /// stdout alone, and stderr only explains a failure.
    #[test]
    fn captured_output_keeps_stdout_and_stderr_apart() {
        let mut data = frame(1, "head ");
        data.extend(frame(2, "warning on stderr "));
        data.extend(frame(1, "tail"));
        assert_eq!(
            demux_stream(&data).unwrap(),
            b"head warning on stderr tail".to_vec()
        );
        let (stdout, stderr) = split_exec_stream(&data).unwrap();
        assert_eq!(stdout, b"head tail".to_vec());
        assert_eq!(stderr, b"warning on stderr ".to_vec());
        assert_eq!(demux_stream(&[]).unwrap(), Vec::<u8>::new());
        assert!(split_exec_stream(&[1, 0, 0, 0, 0, 0, 0, 9, b'x']).is_err());
        assert!(split_exec_stream(&[1, 0, 0]).is_err());
    }

    #[test]
    fn a_pull_error_inside_the_stream_is_reported() {
        assert!(pull_error(br#"{"status":"Pulling from library/nginx","id":"latest"}"#).is_ok());
        assert!(pull_error(b"  ").is_ok());
        let error = pull_error(br#"{"errorDetail":{"message":"denied"},"error":"denied"}"#)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "denied");
        assert!(pull_error(b"not json").is_err());
    }

    #[test]
    fn a_build_stream_yields_the_image_id_or_the_error_with_the_last_lines() {
        let mut output = BuildOutput::default();
        output
            .read(br#"{"stream":"Step 1/2 : FROM busybox\n"}"#)
            .unwrap();
        output.read(br#"{"aux":{"ID":"sha256:feed"}}"#).unwrap();
        output.read(b" ").unwrap();
        assert_eq!(output.id.as_deref(), Some("sha256:feed"));

        let mut output = BuildOutput::default();
        for step in 0..30 {
            let line = json!({ "stream": format!("line {step}\n\n") }).to_string();
            output.read(line.as_bytes()).unwrap();
        }
        let error = output
            .read(br#"{"errorDetail":{"code":1},"error":"exit code 2"}"#)
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("exit code 2\nline 10\n"), "{error}");
        assert!(error.ends_with("line 29"), "{error}");
        assert!(output.read(b"not json").is_err());
    }

    #[test]
    fn the_create_body_carries_the_spec() {
        let spec = ContainerSpec {
            name: "shop-1".parse().unwrap(),
            image: "nginx@sha256:abc".parse().unwrap(),
            env: vec![EnvVar {
                key: "MODE".parse().unwrap(),
                value: "a=b".into(),
            }],
            labels: BTreeMap::from([(APP_LABEL.to_string(), "shop".to_string())]),
            network: Some("r3v3rs3-shop".parse().unwrap()),
            volumes: vec![VolumeMount {
                volume: "shop-data".parse().unwrap(),
                target: "/data".parse().unwrap(),
                read_only: true,
            }],
            restart: RestartPolicy::OnFailure,
            limits: ResourceLimits {
                memory_bytes: Some(256 << 20),
                nano_cpus: None,
                memory_swap_bytes: None,
                pids_limit: Some(1024),
            },
            publish: Some(8080),
            host_port: Some(18080),
            aliases: vec!["r3v3rs3-shop".to_string()],
            extra_networks: vec!["r3v3rs3-db".parse().unwrap()],
            log_max_size: None,
            log_max_file: None,
            command: None,
        };
        let expected = json!({
            "Image": "nginx@sha256:abc",
            "Env": ["MODE=a=b"],
            "Labels": { "r3v3rs3.app": "shop" },
            "HostConfig": {
                "RestartPolicy": { "Name": "on-failure" },
                "Mounts": [{
                    "Type": "volume",
                    "Source": "shop-data",
                    "Target": "/data",
                    "ReadOnly": true,
                }],
                "LogConfig": {
                    "Type": "json-file",
                    "Config": {
                        "max-size": "50m",
                        "max-file": "3"
                    }
                },
                "SecurityOpt": ["no-new-privileges:true"],
                "CapDrop": ["NET_RAW", "MKNOD", "AUDIT_WRITE"],
                "Memory": 268435456,
                "PidsLimit": 1024,
                "NetworkMode": "r3v3rs3-shop",
                "PortBindings": { "8080/tcp": [{ "HostIp": "127.0.0.1", "HostPort": "18080" }] },
            },
            // The extra networks join after the creation, so the body names only `network`.
            "NetworkingConfig": { "EndpointsConfig": {
                "r3v3rs3-shop": { "Aliases": ["r3v3rs3-shop"] },
            } },
            "ExposedPorts": { "8080/tcp": {} },
        });
        assert_eq!(create_body(&spec), expected);
        // Without a fixed host port Docker picks a free one.
        let mut floating = spec;
        floating.host_port = None;
        assert_eq!(
            create_body(&floating)["HostConfig"]["PortBindings"]["8080/tcp"][0]["HostPort"],
            json!("")
        );
    }

    #[test]
    fn an_inspect_document_becomes_container_info() {
        let document = json!({
            "Id": "0123abcd",
            "Name": "/shop-1",
            "Image": "sha256:feed",
            "State": { "Running": true, "ExitCode": 0, "Health": { "Status": "healthy" } },
            "Config": { "Labels": { "r3v3rs3.app": "shop" } },
            "NetworkSettings": { "Networks": {
                "r3v3rs3-shop": { "IPAddress": "172.20.0.2", "GlobalIPv6Address": "" },
                "v6": { "IPAddress": "", "GlobalIPv6Address": "fd00::2" },
                "none": { "IPAddress": "", "GlobalIPv6Address": "" },
            }, "Ports": {
                "80/tcp": [
                    { "HostIp": "0.0.0.0", "HostPort": "8081" },
                    { "HostIp": "127.0.0.1", "HostPort": "49153" },
                ],
                "53/udp": [{ "HostIp": "127.0.0.1", "HostPort": "5353" }],
                "443/tcp": null,
            }},
        });
        let info = serde_json::from_value::<InspectContainer>(document)
            .unwrap()
            .into_info();
        assert_eq!(info.name, "shop-1");
        assert!(info.running);
        assert_eq!(info.health, Some(ContainerHealth::Healthy));
        assert_eq!(
            info.addresses,
            BTreeMap::from([
                ("r3v3rs3-shop".to_string(), "172.20.0.2".to_string()),
                ("v6".to_string(), "fd00::2".to_string()),
            ])
        );
        assert_eq!(info.labels[APP_LABEL], "shop");
        assert_eq!(info.published, BTreeMap::from([(80, 49153)]));
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(
            encode(r#"{"label":["r3v3rs3.app=shop"]}"#),
            "%7B%22label%22%3A%5B%22r3v3rs3.app%3Dshop%22%5D%7D"
        );
        assert_eq!(encode("sha256:abc"), "sha256%3Aabc");
    }
}
