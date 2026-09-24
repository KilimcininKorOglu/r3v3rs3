//! The deployment platform: the apps, their targets and their environment variables.
//!
//! The admin API calls the platform directly instead of through the server loop, because a
//! database query or a Docker call must not hold up the ports and the other RPC methods.

mod agents;
mod build;
mod compose;
pub mod deploy;
#[cfg(test)]
mod fake;
pub mod proxy;
mod publish;
pub mod store;

use crate::agent::pki::AgentPki;
use crate::agent::registry::AgentRegistry;
use crate::build::compose::{ComposeRunner, DockerCompose};
use crate::build::{GitFetcher, SourceFetcher};
use crate::cluster::crypto::ClusterKeys;
use crate::cluster::key_file::{load_keys, write_new_key_file};
use crate::command::ServerCommand;
use crate::kv::http::ApiClient;
use crate::runtime::ContainerRuntime;
use crate::runtime::docker::DockerRuntime;
use anyhow::{Context as _, anyhow};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::discovery::Endpoint;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppLog, AppRequest, DeploymentEntry, EnvEntry, PlatformConfig, TargetEntry,
};
use rand::seq::IndexedRandom;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{GIT_TOKEN, PlatformStore, StoredEnv, Write};
use tokio::sync::{Mutex, Semaphore, broadcast, mpsc};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tracing::error;

/// The database of the platform in the config directory.
pub const DATABASE_FILE: &str = "platform.db";

/// The encryption key of the environment variables in the config directory.
pub const KEY_FILE: &str = "platform.key";

/// The most deployments that one list returns.
pub const DEPLOYMENT_LIST_LIMIT: u32 = 100;

/// The most log lines that one log request returns.
pub const MAX_LOG_TAIL: u32 = 1000;

/// The log lines that a log request returns without a `tail`.
pub const DEFAULT_LOG_TAIL: u32 = 200;

/// The longest value of an environment variable in bytes.
const MAX_ENV_VALUE_LENGTH: usize = 64 * 1024;

/// The longest Git token.
const MAX_GIT_TOKEN_LENGTH: usize = 4096;

/// The state of the platform after the start of the server.
#[derive(Clone, Default)]
pub enum PlatformHandle {
    #[default]
    Disabled,
    InCluster,
    /// The platform failed to open its database or its key. The server log has the reason.
    Failed,
    Ready(Arc<Platform>),
}

impl PlatformHandle {
    /// Opens the platform when `config.toml` enables it, and starts the task that sends the proxies
    /// of the apps to the server through `command`.
    pub async fn start(
        config: &AppConfig,
        config_dir: &Path,
        command: mpsc::Sender<ServerCommand>,
        events: &broadcast::Sender<ServerEvent>,
    ) -> Self {
        if !config.platform.enabled {
            return Self::Disabled;
        }
        if config.cluster.enabled {
            error!("the deployment platform does not run in a cluster, so it stays off");
            return Self::InCluster;
        }
        match Platform::open(config, config_dir, command).await {
            Ok(platform) => {
                let platform = Arc::new(platform);
                tokio::spawn(publish::refresh(Arc::downgrade(&platform)));
                if let Some(port) = config.platform.agent_port
                    && let Err(err) = platform
                        .start_agent_listener(port, events.subscribe())
                        .await
                {
                    // The local target keeps working without the agents.
                    error!(err = format!("{err:#}"), "failed to start the agent port");
                }
                Self::Ready(platform)
            }
            Err(err) => {
                error!(
                    err = format!("{err:#}"),
                    "failed to start the deployment platform"
                );
                Self::Failed
            }
        }
    }

    pub fn get(&self) -> Result<&Arc<Platform>, Error> {
        match self {
            Self::Disabled => Err(Error::PlatformDisabled),
            Self::InCluster => Err(Error::PlatformInCluster),
            Self::Failed => Err(Error::PlatformFailed),
            Self::Ready(platform) => Ok(platform),
        }
    }
}

pub struct Platform {
    store: PlatformStore,
    keys: ClusterKeys,
    local: Arc<dyn ContainerRuntime>,
    config: PlatformConfig,
    command: mpsc::Sender<ServerCommand>,
    /// The proxies and the issues of the last snapshot that the server got.
    sent: Mutex<Option<publish::Published>>,
    /// The apps that a pipeline or a deletion holds.
    busy: std::sync::Mutex<HashSet<ShortId>>,
    timing: deploy::Timing,
    fetcher: Arc<dyn SourceFetcher>,
    /// Limits the builds that run at the same time.
    builds: Semaphore,
    /// The directory of the checkouts.
    build_dir: PathBuf,
    compose: Arc<dyn ComposeRunner>,
    /// The directory of the Compose checkouts.
    compose_dir: PathBuf,
    /// The CA of the agent link. Only a server with an agent port has it.
    pki: Option<AgentPki>,
    /// The connected agents.
    agents: Arc<AgentRegistry>,
}

impl Platform {
    async fn open(
        config: &AppConfig,
        config_dir: &Path,
        command: mpsc::Sender<ServerCommand>,
    ) -> anyhow::Result<Self> {
        let local = docker_runtime(&config.platform.docker)?;
        let store = PlatformStore::open(&config_dir.join(DATABASE_FILE)).await?;
        let now = crate::clock::unix_ms();
        store.ensure_local_target(now).await?;
        store.fail_unfinished(now).await?;
        let keys = open_keys(config_dir).await?;
        // A stopped server can leave the checkouts of its unfinished builds.
        let build_dir = config_dir.join(build::BUILD_DIR);
        build::remove_dir(&build_dir).await?;
        let pki = agent_pki(&config.platform, config_dir).await?;
        Ok(Self {
            pki,
            agents: Arc::new(AgentRegistry::default()),
            store,
            keys,
            local: Arc::new(local),
            config: config.platform.clone(),
            command,
            sent: Mutex::new(None),
            busy: std::sync::Mutex::new(HashSet::new()),
            timing: deploy::Timing::default(),
            fetcher: Arc::new(GitFetcher),
            builds: Semaphore::new(build::MAX_BUILDS),
            build_dir,
            compose: Arc::new(DockerCompose::new(config.platform.docker.clone())),
            compose_dir: config_dir.join(compose::COMPOSE_DIR),
        })
    }

    fn busy_apps(&self) -> std::sync::MutexGuard<'_, HashSet<ShortId>> {
        match self.busy.lock() {
            Ok(busy) => busy,
            // The set stays valid when a holder panics.
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// The runtime of the local target.
    pub fn local_runtime(&self) -> Arc<dyn ContainerRuntime> {
        self.local.clone()
    }

    pub async fn targets(&self) -> anyhow::Result<Vec<TargetEntry>> {
        self.store.targets().await
    }

    pub async fn apps(&self) -> anyhow::Result<Vec<AppEntry>> {
        self.store.apps().await
    }

    pub async fn app(&self, id: ShortId) -> anyhow::Result<AppEntry> {
        self.store.app(id).await?.ok_or_else(|| not_found(id))
    }

    pub async fn add_app(&self, request: AppRequest, now: u64) -> anyhow::Result<AppEntry> {
        self.check_request(&request).await?;
        let app = AppEntry {
            id: self.free_id().await?,
            name: request.name,
            target: request.target,
            spec: request.spec,
            git_token_set: false,
            created_at: now,
            updated_at: now,
        };
        match self.store.insert_app(&app).await? {
            Write::Done => Ok(app),
            Write::NameTaken => Err(name_taken(&app).into()),
            Write::NotFound => Err(anyhow!("the app was not stored")),
        }
    }

    /// A new id that no app uses.
    async fn free_id(&self) -> anyhow::Result<ShortId> {
        for _ in 0..16 {
            let id = new_id()?;
            if self.store.app(id).await?.is_none() {
                return Ok(id);
            }
        }
        Err(anyhow!("no free app id"))
    }

    pub async fn update_app(
        &self,
        id: ShortId,
        request: AppRequest,
        now: u64,
    ) -> anyhow::Result<AppEntry> {
        self.check_request(&request).await?;
        let current = self.app(id).await?;
        let app = AppEntry {
            id,
            name: request.name,
            target: request.target,
            spec: request.spec,
            git_token_set: current.git_token_set,
            created_at: current.created_at,
            updated_at: now,
        };
        match self.store.update_app(&app).await? {
            Write::Done => Ok(app),
            Write::NameTaken => Err(name_taken(&app).into()),
            Write::NotFound => Err(not_found(id)),
        }
    }

    /// Deletes an app with its containers, its network, its environment and its deployments.
    pub async fn delete_app(self: &Arc<Self>, id: ShortId) -> anyhow::Result<AppEntry> {
        let app = self.app(id).await?;
        let _lock = self.lock_app(&app)?;
        // Only a pipeline creates containers, so an app without deployments needs no Docker call.
        if !self.store.deployments(id, 1).await?.is_empty() {
            self.remove_app_resources(id).await?;
        }
        if !self.store.delete_app(id).await? {
            return Err(not_found(id));
        }
        self.publish().await;
        Ok(app)
    }

    /// The environment variables of an app without the values of the secret variables.
    pub async fn env(&self, id: ShortId) -> anyhow::Result<Vec<EnvEntry>> {
        self.app(id).await?;
        let stored = self.store.env(id).await?;
        stored
            .into_iter()
            .map(|var| {
                let value = if var.secret {
                    None
                } else {
                    Some(self.open_value(id, &var)?)
                };
                Ok(EnvEntry {
                    key: var.key.parse()?,
                    value,
                    secret: var.secret,
                })
            })
            .collect()
    }

    /// Replaces the environment variables of an app. A secret variable without a value keeps
    /// its current value.
    pub async fn set_env(&self, id: ShortId, entries: Vec<EnvEntry>) -> anyhow::Result<()> {
        self.app(id).await?;
        let current = self
            .store
            .env(id)
            .await?
            .into_iter()
            .map(|var| (var.key.clone(), var))
            .collect::<BTreeMap<_, _>>();
        let mut keys = HashSet::new();
        let mut stored = Vec::with_capacity(entries.len());
        for entry in entries {
            if !keys.insert(entry.key.as_str().to_string()) {
                return Err(invalid_env(&format!(
                    "{} is listed twice",
                    entry.key.as_str()
                )));
            }
            stored.push(self.stored_env(id, entry, &current)?);
        }
        self.store.replace_env(id, &stored).await
    }

    /// Sets the token that clones the private repository of an app, and returns the app.
    pub async fn set_git_token(&self, id: ShortId, token: &str) -> anyhow::Result<AppEntry> {
        self.app(id).await?;
        check_git_token(token)?;
        let sealed = self.keys.seal(&git_token_aad(id), token.as_bytes())?;
        self.store.set_secret(id, GIT_TOKEN, &sealed).await?;
        self.app(id).await
    }

    /// Deletes the Git token of an app, and returns the app.
    pub async fn delete_git_token(&self, id: ShortId) -> anyhow::Result<AppEntry> {
        self.app(id).await?;
        self.store.delete_secret(id, GIT_TOKEN).await?;
        self.app(id).await
    }

    /// The Git token of an app, opened for one clone.
    async fn git_token(&self, id: ShortId) -> anyhow::Result<Option<String>> {
        let Some(sealed) = self.store.secret(id, GIT_TOKEN).await? else {
            return Ok(None);
        };
        let token = self.keys.open(&git_token_aad(id), &sealed)?;
        Ok(Some(
            String::from_utf8(token).context("the Git token is not UTF-8")?,
        ))
    }

    /// The latest deployments of an app, the newest first.
    pub async fn deployments(&self, id: ShortId) -> anyhow::Result<Vec<DeploymentEntry>> {
        self.app(id).await?;
        self.store.deployments(id, DEPLOYMENT_LIST_LIMIT).await
    }

    /// The last `tail` lines of the container log of the running deployment of an app. An app
    /// without a running deployment has an empty log.
    pub async fn app_log(&self, id: ShortId, tail: u32) -> anyhow::Result<AppLog> {
        self.app(id).await?;
        let Some(deployment) = self.store.running_deployment(id).await? else {
            return Ok(AppLog {
                log: String::new(),
                running: false,
            });
        };
        let name = proxy::container_name(id, deployment)?;
        let log = self.local.logs(&name, tail.clamp(1, MAX_LOG_TAIL)).await?;
        Ok(AppLog { log, running: true })
    }

    pub async fn deployment(&self, id: ShortId) -> anyhow::Result<DeploymentEntry> {
        self.store
            .deployment(id)
            .await?
            .ok_or_else(|| not_found(id))
    }

    async fn check_request(&self, request: &AppRequest) -> anyhow::Result<()> {
        request.spec.validate()?;
        if !self.store.has_target(request.target).await? {
            return Err(not_found(request.target));
        }
        Ok(())
    }

    fn stored_env(
        &self,
        id: ShortId,
        entry: EnvEntry,
        current: &BTreeMap<String, StoredEnv>,
    ) -> anyhow::Result<StoredEnv> {
        let key = entry.key.as_str().to_string();
        let sealed = match (entry.value, current.get(&key)) {
            (Some(value), _) => {
                check_env_value(&key, &value)?;
                self.keys.seal(&env_aad(id, &key), value.as_bytes())?
            }
            // Only a secret variable keeps its value, so a variable does not turn secret with
            // the value that the API showed before.
            (None, Some(saved)) if entry.secret && saved.secret => saved.sealed.clone(),
            (None, _) => return Err(invalid_env(&format!("{key} needs a value"))),
        };
        Ok(StoredEnv {
            key,
            sealed,
            secret: entry.secret,
        })
    }

    fn open_value(&self, id: ShortId, var: &StoredEnv) -> anyhow::Result<String> {
        let value = self.keys.open(&env_aad(id, &var.key), &var.sealed)?;
        String::from_utf8(value).context("an environment value is not UTF-8")
    }
}

/// The associated data of a sealed value binds it to its app and its key, so a value copied to
/// another row does not open.
fn env_aad(app: ShortId, key: &str) -> String {
    format!("app/{app}/env/{key}")
}

fn git_token_aad(app: ShortId) -> String {
    format!("app/{app}/secret/{GIT_TOKEN}")
}

/// A token is one word of printable ASCII, because git sends it in an HTTP header.
fn check_git_token(token: &str) -> anyhow::Result<()> {
    let valid = !token.is_empty()
        && token.len() <= MAX_GIT_TOKEN_LENGTH
        && token.chars().all(|c| c.is_ascii_graphic());
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidContainerSpec {
            reason: "a Git token is up to 4096 printable ASCII characters without spaces"
                .to_string(),
        }
        .into())
    }
}

fn check_env_value(key: &str, value: &str) -> anyhow::Result<()> {
    if value.contains('\0') {
        return Err(invalid_env(&format!(
            "the value of {key} contains a NUL character"
        )));
    }
    if value.len() > MAX_ENV_VALUE_LENGTH {
        return Err(invalid_env(&format!("the value of {key} is too long")));
    }
    Ok(())
}

fn invalid_env(reason: &str) -> anyhow::Error {
    Error::InvalidContainerSpec {
        reason: reason.to_string(),
    }
    .into()
}

fn not_found(id: ShortId) -> anyhow::Error {
    Error::IdNotFound { id: id.to_string() }.into()
}

fn name_taken(app: &AppEntry) -> Error {
    Error::AppNameExists {
        name: app.name.to_string(),
    }
}

/// A new id in the form of the other ids of the server, for example `bcd-fgh`.
fn new_id() -> anyhow::Result<ShortId> {
    const TABLE: &[u8] = b"bcdfghjklmnpqrstvwxyz";
    let mut rng = rand::rng();
    let mut id = String::with_capacity(7);
    for index in 0..6 {
        if index == 3 {
            id.push('-');
        }
        let c = TABLE.choose(&mut rng).context("the id table is empty")?;
        id.push(char::from(*c));
    }
    Ok(id.parse()?)
}

/// The key of the environment values, created at the first start.
async fn open_keys(config_dir: &Path) -> anyhow::Result<ClusterKeys> {
    let key_path = config_dir.join(KEY_FILE);
    if !tokio::fs::try_exists(&key_path).await? {
        write_new_key_file(&key_path).await?;
    }
    load_keys(std::slice::from_ref(&key_path)).await
}

/// The CA of the agent link. Only a server with an agent port needs it.
async fn agent_pki(config: &PlatformConfig, config_dir: &Path) -> anyhow::Result<Option<AgentPki>> {
    match config.agent_port {
        Some(_) => Ok(Some(AgentPki::load_or_create(config_dir).await?)),
        None => Ok(None),
    }
}

/// The runtime of the Docker Engine at `endpoint`. The platform talks to a local engine over a
/// Unix socket or plain TCP.
fn docker_runtime(endpoint: &str) -> anyhow::Result<DockerRuntime> {
    let endpoint = endpoint
        .parse::<Endpoint>()
        .map_err(|err| anyhow!("invalid Docker endpoint {endpoint}: {err}"))?;
    if matches!(endpoint, Endpoint::Tcp { tls: true, .. }) {
        return Err(anyhow!(
            "the Docker endpoint must be a Unix socket or plain TCP"
        ));
    }
    let tls = ClientConfig::builder()
        .with_root_certificates(RootCertStore::empty())
        .with_no_client_auth();
    Ok(DockerRuntime::new(ApiClient::new(
        vec![endpoint],
        Arc::new(tls),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::platform::{AppSource, AppSpec, LOCAL_TARGET};
    use std::time::Duration;

    pub(super) struct TempDir(pub(super) std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(super) async fn platform() -> anyhow::Result<(Arc<Platform>, TempDir)> {
        // These tests read no commands.
        let (command, _) = mpsc::channel(1);
        let (platform, _, dir) = platform_with(&PlatformConfig::default(), command).await?;
        Ok((Arc::new(platform), dir))
    }

    /// A platform on a runtime in memory, with short deployment durations.
    pub(super) async fn platform_with(
        platform: &PlatformConfig,
        command: mpsc::Sender<ServerCommand>,
    ) -> anyhow::Result<(Platform, Arc<fake::FakeRuntime>, TempDir)> {
        let dir = TempDir(std::env::temp_dir().join(format!(
            "r3v3rs3-platform-{}",
            hex::encode(rand::random::<[u8; 8]>())
        )));
        std::fs::create_dir_all(&dir.0)?;
        let config = AppConfig {
            platform: PlatformConfig {
                enabled: true,
                ..platform.clone()
            },
            ..Default::default()
        };
        let mut platform = Platform::open(&config, &dir.0, command).await?;
        let runtime = Arc::new(fake::FakeRuntime::default());
        platform.local = runtime.clone();
        platform.timing = deploy::Timing {
            health_timeout: Duration::from_secs(2),
            health_interval: Duration::from_millis(20),
            probe_timeout: Duration::from_secs(1),
            drain: Duration::ZERO,
            stop_timeout: Duration::ZERO,
        };
        Ok((platform, runtime, dir))
    }

    pub(super) fn request(name: &str) -> AppRequest {
        AppRequest {
            name: name.parse().unwrap(),
            target: LOCAL_TARGET.parse().unwrap(),
            spec: AppSpec {
                source: AppSource::Image {
                    image: "nginx:1.27".parse().unwrap(),
                },
                port: 80,
                domains: Vec::new(),
                health_check_path: None,
                volumes: Vec::new(),
                restart: Default::default(),
                limits: Default::default(),
            },
        }
    }

    fn env(key: &str, value: Option<&str>, secret: bool) -> EnvEntry {
        EnvEntry {
            key: key.parse().unwrap(),
            value: value.map(str::to_string),
            secret,
        }
    }

    fn api_error(err: anyhow::Error) -> Error {
        err.downcast::<Error>().expect("an API error")
    }

    #[tokio::test]
    async fn apps_are_added_renamed_and_deleted() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let shop = platform.add_app(request("shop"), 1).await?;
        let err = platform.add_app(request("shop"), 2).await.unwrap_err();
        assert!(matches!(api_error(err), Error::AppNameExists { .. }));

        let mut missing_target = request("blog");
        missing_target.target = "nowhere".parse()?;
        let err = platform.add_app(missing_target, 2).await.unwrap_err();
        assert!(matches!(api_error(err), Error::IdNotFound { .. }));

        let renamed = platform.update_app(shop.id, request("store"), 5).await?;
        assert_eq!((renamed.created_at, renamed.updated_at), (1, 5));
        assert_eq!(platform.apps().await?, vec![renamed.clone()]);

        platform.delete_app(shop.id).await?;
        let err = platform.app(shop.id).await.unwrap_err();
        assert!(matches!(api_error(err), Error::IdNotFound { .. }));
        Ok(())
    }

    #[tokio::test]
    async fn secret_values_are_sealed_hidden_and_kept() -> anyhow::Result<()> {
        let (platform, dir) = platform().await?;
        let shop = platform.add_app(request("shop"), 1).await?;
        let first = vec![
            env("MODE", Some("production"), false),
            env("TOKEN", Some("s3cr3t-value"), true),
        ];
        platform.set_env(shop.id, first).await?;
        assert_eq!(
            platform.env(shop.id).await?,
            vec![
                env("MODE", Some("production"), false),
                env("TOKEN", None, true),
            ]
        );

        // The database holds only sealed values.
        let database = std::fs::read(dir.0.join(DATABASE_FILE))?;
        let needle = b"s3cr3t-value";
        assert!(
            !database
                .windows(needle.len())
                .any(|window| window == needle)
        );

        // An update without the secret value keeps it.
        let update = vec![
            env("MODE", Some("staging"), false),
            env("TOKEN", None, true),
        ];
        platform.set_env(shop.id, update).await?;
        let stored = platform.store.env(shop.id).await?;
        let token = stored.iter().find(|var| var.key == "TOKEN").unwrap();
        assert_eq!(platform.open_value(shop.id, token)?, "s3cr3t-value");

        // A value sealed for one app does not open for another app.
        let blog = platform.add_app(request("blog"), 2).await?;
        assert!(platform.open_value(blog.id, token).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn an_env_update_needs_values_and_unique_keys() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let shop = platform.add_app(request("shop"), 1).await?;
        let cases = [
            vec![env("A", None, false)],
            vec![env("A", None, true)],
            vec![env("A", Some("1"), false), env("A", Some("2"), false)],
            vec![env("A", Some("a\0b"), false)],
        ];
        for entries in cases {
            let err = platform
                .set_env(shop.id, entries.clone())
                .await
                .unwrap_err();
            assert!(
                matches!(api_error(err), Error::InvalidContainerSpec { .. }),
                "{entries:?}"
            );
        }
        // A plain variable cannot turn secret without a new value.
        platform
            .set_env(shop.id, vec![env("A", Some("1"), false)])
            .await?;
        assert!(
            platform
                .set_env(shop.id, vec![env("A", None, true)])
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_platform_stays_off_unless_enabled_and_outside_a_cluster() {
        let dir = std::env::temp_dir();
        let (command, _commands) = mpsc::channel(1);
        let (events, _) = broadcast::channel(1);
        let mut config = AppConfig::default();
        let handle = PlatformHandle::start(&config, &dir, command.clone(), &events).await;
        assert!(matches!(handle.get(), Err(Error::PlatformDisabled)));

        config.platform.enabled = true;
        config.cluster.enabled = true;
        let handle = PlatformHandle::start(&config, &dir, command.clone(), &events).await;
        assert!(matches!(handle.get(), Err(Error::PlatformInCluster)));

        config.cluster.enabled = false;
        config.platform.docker = "https://docker.example:2376".into();
        let handle = PlatformHandle::start(&config, &dir, command, &events).await;
        assert!(matches!(handle.get(), Err(Error::PlatformFailed)));
    }
}
