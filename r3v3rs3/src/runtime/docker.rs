//! The local runtime: the Docker Engine API over a Unix socket or TCP.
//!
//! Docker finds a container or a network by its id, then by its name, then by an id prefix. A name
//! that does not exist can therefore match another object whose id starts with the same hex
//! characters. Every call that changes an object first inspects it by name, checks that the name
//! matches, and then uses the returned id.

use super::ContainerRuntime;
use crate::kv::http::{ApiClient, Lines, RESPONSE_TIMEOUT, read_body, read_json};
use anyhow::{anyhow, bail};
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::CONTENT_TYPE;
use hyper::{Method, Response, StatusCode};
use r3v3rs3_api::container::{
    APP_LABEL, AppName, ContainerHealth, ContainerInfo, ContainerName, ContainerSpec,
    ContainerSummary, ImageInfo, ImageRef, NetworkName,
};
use r3v3rs3_api::git::RelPath;
use serde::de::DeserializeOwned;
use serde_derive::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

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

#[derive(Clone)]
pub struct DockerRuntime {
    client: ApiClient,
}

impl DockerRuntime {
    pub fn new(client: ApiClient) -> Self {
        Self { client }
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
}

#[async_trait::async_trait]
impl ContainerRuntime for DockerRuntime {
    async fn version(&self) -> anyhow::Result<String> {
        let version: VersionResponse = self.get("/version").await?;
        Ok(version.version)
    }

    async fn pull_image(&self, image: &ImageRef) -> anyhow::Result<()> {
        let path = format!(
            "/images/create?fromImage={}&tag={}",
            encode(image.repository()),
            encode(image.tag_or_digest())
        );
        let response = self
            .send(Method::POST, &path, None, &[], RESPONSE_TIMEOUT)
            .await?;
        let mut lines = Lines::new(response);
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

    async fn remove_image(&self, image: &ImageRef) -> anyhow::Result<()> {
        let path = format!("/images/{image}");
        // 409 Conflict: a container still uses the image, so it stays until a later cleanup.
        let allowed = [StatusCode::NOT_FOUND, StatusCode::CONFLICT];
        self.call(Method::DELETE, &path, None, &allowed).await
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
    let mut host_config = json!({
        "RestartPolicy": { "Name": spec.restart.docker_name() },
        "Mounts": mounts,
    });
    if let Some(memory) = spec.limits.memory_bytes {
        host_config["Memory"] = json!(memory);
    }
    if let Some(cpus) = spec.limits.nano_cpus {
        host_config["NanoCpus"] = json!(cpus);
    }
    let mut body = json!({
        "Image": spec.image.as_str(),
        "Env": env,
        "Labels": spec.labels,
        "HostConfig": host_config,
    });
    if let Some(network) = &spec.network {
        body["HostConfig"]["NetworkMode"] = json!(network.as_str());
        body["NetworkingConfig"] = json!({ "EndpointsConfig": { network.as_str(): {} } });
    }
    if let Some(port) = spec.publish {
        let key = format!("{port}/tcp");
        body["ExposedPorts"] = json!({ key.as_str(): {} });
        // An empty host port lets Docker pick a free port.
        body["HostConfig"]["PortBindings"] =
            json!({ key.as_str(): [{ "HostIp": PUBLISH_HOST, "HostPort": "" }] });
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
fn demux_logs(mut data: &[u8]) -> anyhow::Result<String> {
    let mut output = Vec::with_capacity(data.len());
    while !data.is_empty() {
        let header = data
            .get(..8)
            .ok_or_else(|| anyhow!("a log frame header is truncated"))?;
        let size = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let payload = data
            .get(8..8 + size)
            .ok_or_else(|| anyhow!("a log frame is truncated"))?;
        output.extend_from_slice(payload);
        data = &data[8 + size..];
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
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
    config: Option<InspectConfig>,
    #[serde(default)]
    network_settings: Option<InspectNetworkSettings>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectState {
    running: bool,
    exit_code: i64,
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
            },
            publish: Some(8080),
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
                "Memory": 268435456,
                "NetworkMode": "r3v3rs3-shop",
                "PortBindings": { "8080/tcp": [{ "HostIp": "127.0.0.1", "HostPort": "" }] },
            },
            "NetworkingConfig": { "EndpointsConfig": { "r3v3rs3-shop": {} } },
            "ExposedPorts": { "8080/tcp": {} },
        });
        assert_eq!(create_body(&spec), expected);
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
