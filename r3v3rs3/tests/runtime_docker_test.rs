//! The Docker runtime against a real Docker Engine: pull, network, container lifecycle, logs and
//! removal.
//!
//! The test needs a Docker Engine. It reads `DOCKER_HOST` and falls back to
//! `unix:///var/run/docker.sock`. `make test-runtime-docker` runs it.

use anyhow::{Context as _, ensure};
use r3v3rs3::kv::http::ApiClient;
use r3v3rs3::runtime::ContainerRuntime;
use r3v3rs3::runtime::docker::DockerRuntime;
use r3v3rs3_api::container::{
    APP_LABEL, AppName, ContainerName, ContainerSpec, EnvVar, ImageRef, NetworkName,
    ResourceLimits, RestartPolicy,
};
use r3v3rs3_api::discovery::Endpoint;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

/// A small multi-platform image that serves HTTP on port 80 with its default command.
const IMAGE: &str = "traefik/whoami:v1.10.3";

fn runtime() -> anyhow::Result<DockerRuntime> {
    let host = std::env::var("DOCKER_HOST").unwrap_or("unix:///var/run/docker.sock".into());
    let endpoint = host.parse::<Endpoint>()?;
    let tls = ClientConfig::builder()
        .with_root_certificates(RootCertStore::empty())
        .with_no_client_auth();
    Ok(DockerRuntime::new(ApiClient::new(
        vec![endpoint],
        Arc::new(tls),
    )))
}

/// `assert_eq!` for a function that returns `anyhow::Result`, so a failure still reaches the
/// cleanup of the test.
macro_rules! ensure_eq {
    ($left:expr, $right:expr $(,)?) => {{
        let (left, right) = (&$left, &$right);
        ensure!(left == right, "{:?} != {:?}", left, right);
    }};
}

struct Names {
    app: AppName,
    network: NetworkName,
    container: ContainerName,
}

impl Names {
    fn new() -> anyhow::Result<Self> {
        let app = format!("r3v3rs3-rt-{:08x}", rand::random::<u32>());
        Ok(Self {
            network: app.parse()?,
            container: format!("{app}-web").parse()?,
            app: app.parse()?,
        })
    }
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_container_runs_through_its_lifecycle() -> anyhow::Result<()> {
    let runtime = runtime()?;
    let names = Names::new()?;
    let result = lifecycle(&runtime, &names).await;
    // Clean up whether the checks passed or not, then report the first failure.
    let container = runtime.remove_container(&names.container).await;
    let network = runtime.remove_network(&names.network).await;
    result?;
    container?;
    network
}

async fn lifecycle(runtime: &DockerRuntime, names: &Names) -> anyhow::Result<()> {
    ensure!(!runtime.version().await?.is_empty());
    let image = pull(runtime).await?;
    runtime.ensure_network(&names.network, &names.app).await?;
    runtime.ensure_network(&names.network, &names.app).await?;
    let id = create_and_start(runtime, names, image).await?;
    check_running(runtime, names, &id).await?;
    check_prefix_is_not_a_name(runtime, &id).await?;
    stop_and_remove(runtime, names).await
}

async fn pull(runtime: &DockerRuntime) -> anyhow::Result<ImageRef> {
    let image: ImageRef = IMAGE.parse()?;
    runtime.pull_image(&image).await?;
    let info = runtime
        .inspect_image(&image)
        .await?
        .context("pulled image")?;
    ensure!(
        info.repo_digests
            .iter()
            .any(|digest| digest.starts_with("traefik/whoami@sha256:")),
        "{:?}",
        info.repo_digests
    );
    Ok(image)
}

fn spec(names: &Names, image: ImageRef) -> anyhow::Result<ContainerSpec> {
    Ok(ContainerSpec {
        name: names.container.clone(),
        image,
        env: vec![EnvVar {
            key: "WHOAMI_NAME".parse()?,
            value: "r3v3rs3".into(),
        }],
        labels: BTreeMap::from([(APP_LABEL.to_string(), names.app.to_string())]),
        network: Some(names.network.clone()),
        volumes: Vec::new(),
        restart: RestartPolicy::No,
        limits: ResourceLimits {
            memory_bytes: Some(64 << 20),
            nano_cpus: Some(500_000_000),
        },
    })
}

/// Creates and starts the container, and returns its id.
async fn create_and_start(
    runtime: &DockerRuntime,
    names: &Names,
    image: ImageRef,
) -> anyhow::Result<String> {
    let spec = spec(names, image)?;
    let id = runtime.create_container(&spec).await?;
    ensure!(
        runtime.create_container(&spec).await.is_err(),
        "a second container with the same name must fail"
    );
    runtime.start_container(&names.container).await?;
    runtime.start_container(&names.container).await?;
    Ok(id)
}

async fn check_running(runtime: &DockerRuntime, names: &Names, id: &str) -> anyhow::Result<()> {
    let running = runtime
        .inspect_container(&names.container)
        .await?
        .context("created container")?;
    ensure_eq!(running.id, id);
    ensure!(running.running);
    ensure_eq!(
        running.labels.get(APP_LABEL).map(String::as_str),
        Some(names.app.as_str())
    );
    ensure!(
        running.addresses.contains_key(names.network.as_str()),
        "{:?}",
        running.addresses
    );
    let list = runtime.list_containers(&names.app).await?;
    let names_and_states = list
        .iter()
        .map(|container| (container.name.as_str(), container.state.as_str()))
        .collect::<Vec<_>>();
    ensure_eq!(names_and_states, [(names.container.as_str(), "running")]);
    let logs = runtime.logs(&names.container, 50).await?;
    ensure!(logs.contains("80"), "{logs}");
    Ok(())
}

/// A name that is only a prefix of the container id must not find the container.
async fn check_prefix_is_not_a_name(runtime: &DockerRuntime, id: &str) -> anyhow::Result<()> {
    let prefix: ContainerName = id[..12].parse()?;
    ensure!(runtime.inspect_container(&prefix).await?.is_none());
    ensure!(runtime.start_container(&prefix).await.is_err());
    Ok(())
}

async fn stop_and_remove(runtime: &DockerRuntime, names: &Names) -> anyhow::Result<()> {
    let timeout = Duration::from_secs(5);
    runtime.stop_container(&names.container, timeout).await?;
    runtime.stop_container(&names.container, timeout).await?;
    let stopped = runtime
        .inspect_container(&names.container)
        .await?
        .context("stopped container")?;
    ensure!(!stopped.running);
    runtime.remove_container(&names.container).await?;
    runtime.remove_container(&names.container).await?;
    ensure!(runtime.inspect_container(&names.container).await?.is_none());
    ensure!(runtime.start_container(&names.container).await.is_err());
    Ok(())
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_missing_image_fails_the_pull() -> anyhow::Result<()> {
    let runtime = runtime()?;
    let image: ImageRef = "r3v3rs3/does-not-exist:missing".parse()?;
    let error = runtime.pull_image(&image).await.unwrap_err().to_string();
    assert!(error.contains("r3v3rs3/does-not-exist"), "{error}");
    assert!(runtime.inspect_image(&image).await?.is_none());
    Ok(())
}
