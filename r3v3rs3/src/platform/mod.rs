//! The deployment platform: the apps, their targets and their environment variables.
//!
//! The admin API calls the platform directly instead of through the server loop, because a
//! database query or a Docker call must not hold up the ports and the other RPC methods.

mod agents;
mod app_hooks;
mod backup;
mod build;
mod compose;
mod connections;
pub mod deploy;
pub mod disaster_recovery;
#[cfg(test)]
pub(crate) mod fake;
mod github_app;
pub mod hooks;
pub mod image_check;
mod metrics;
mod notices;
pub mod oauth;
pub mod projects;
mod provider;
pub mod proxy;
pub mod prune;
mod publish;
pub mod registries;
#[cfg(test)]
mod remote_tests;
mod repositories;
pub mod rotate;
mod s3;
pub mod scheduler;
pub mod store;
pub mod volume_backup;
pub mod webhook;

use crate::agent::compose::RemoteCompose;
use crate::agent::forward::Forwarders;
use crate::agent::pki::AgentPki;
use crate::agent::registry::AgentRegistry;
use crate::agent::remote::RemoteRuntime;
use crate::audit::AuditLog;
use crate::build::compose::{ComposeRunner, DockerCompose};
use crate::build::{GitCredential, GitFetcher, SourceFetcher};
use crate::command::ServerCommand;
use crate::keys::crypto::ClusterKeys;
use crate::keys::key_file::{load_keys, write_new_key_file};
use crate::kv::http::ApiClient;
use crate::runtime::ContainerRuntime;
use crate::runtime::docker::{DockerRuntime, ExecHandle};
use anyhow::{Context as _, anyhow};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::container::{COMPOSE_SERVICE_LABEL, ContainerName, ContainerSummary};
use r3v3rs3_api::discovery::Endpoint;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::git::GitRef;
use r3v3rs3_api::git_connection::PublicUrl;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppLog, AppRequest, ComposeServiceStatus, DeploymentEntry, EnvEntry, EnvGroup,
    EnvGroupSpec, EnvGroupSummary, FileEntry, ImageUpdateStatus, LOCAL_TARGET, PlatformConfig,
};
use rand::seq::IndexedRandom;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{GIT_TOKEN, PlatformStore, StoredEnv, StoredEnvGroup, Write};
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
        audit: Arc<AuditLog>,
    ) -> Self {
        if !config.platform.enabled {
            return Self::Disabled;
        }
        match Platform::open(config, config_dir, command, Some(audit)).await {
            Ok(platform) => {
                let platform = Arc::new(platform);
                tokio::spawn(publish::refresh(Arc::downgrade(&platform)));
                backup::spawn_scheduler(platform.clone());
                metrics::spawn_collector(&platform);
                image_check::spawn_checker(&platform);
                scheduler::spawn_command_scheduler(&platform);
                prune::spawn_prune_scheduler(&platform);
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
            Self::Failed => Err(Error::PlatformFailed),
            Self::Ready(platform) => Ok(platform),
        }
    }
}

/// The runtime and the Compose runner of one target.
pub(super) struct Backend {
    pub runtime: Arc<dyn ContainerRuntime>,
    pub compose: Arc<dyn ComposeRunner>,
}

pub struct Platform {
    store: PlatformStore,
    keys: std::sync::RwLock<ClusterKeys>,
    local: Arc<dyn ContainerRuntime>,
    config: PlatformConfig,
    config_dir: PathBuf,
    command: mpsc::Sender<ServerCommand>,
    /// The proxies and the issues of the last snapshot that the server got.
    sent: Mutex<Option<publish::Published>>,
    /// The apps that a pipeline or a deletion holds.
    busy: std::sync::Mutex<HashSet<ShortId>>,
    /// The apps that a webhook request deploys again after their running pipeline, with optional git ref.
    queued: std::sync::Mutex<HashMap<ShortId, Option<GitRef>>>,
    timing: deploy::Timing,
    fetcher: Arc<dyn SourceFetcher>,
    /// Limits the builds that run at the same time.
    builds: Semaphore,
    /// The directory of the checkouts.
    build_dir: PathBuf,
    compose: Arc<dyn ComposeRunner>,
    /// The directory of the Compose checkouts.
    compose_dir: PathBuf,
    /// The directory of the database backups.
    backup_dir: PathBuf,
    /// The CA of the agent link. Only a server with an agent port has it.
    pki: Option<AgentPki>,
    /// The connected agents.
    agents: Arc<AgentRegistry>,
    /// The loopback listeners of the published ports on the agents.
    forwarders: Arc<Forwarders>,
    /// Records the enrollments of the agents. The tests run without it.
    audit: Option<Arc<AuditLog>>,
    /// The HTTPS client of the Git provider APIs, built at its first use.
    http: tokio::sync::OnceCell<crate::cdn::fetch::HttpClient>,
    /// The OAuth authorizations that wait for the callback of their provider.
    pending: oauth::PendingAuthorizations,
    /// Lets one token renewal run at a time.
    renewing: Mutex<()>,
    /// The public address that the Git providers send the webhook requests of the apps to.
    public_url: std::sync::RwLock<Option<PublicUrl>>,
    /// Records the last auto-upgrade attempt time for each target to avoid repeated attempts.
    agent_upgrade_attempts: std::sync::Mutex<HashMap<ShortId, std::time::Instant>>,
}

/// The public address of the config, or the address that an earlier release stored in the
/// platform database when the config does not set one.
async fn resolve_public_url(
    config: &AppConfig,
    store: &PlatformStore,
) -> anyhow::Result<Option<PublicUrl>> {
    match &config.public_url {
        Some(url) => Ok(Some(url.clone())),
        None => store
            .setting(store::PUBLIC_URL)
            .await?
            .map(|url| url.parse())
            .transpose()
            .map_err(Into::into),
    }
}

impl Platform {
    async fn open(
        config: &AppConfig,
        config_dir: &Path,
        command: mpsc::Sender<ServerCommand>,
        audit: Option<Arc<AuditLog>>,
    ) -> anyhow::Result<Self> {
        let local = docker_runtime(&config.platform.docker)?;
        let store = PlatformStore::open(&config_dir.join(DATABASE_FILE)).await?;
        let now = crate::clock::unix_ms();
        store.ensure_local_target(now).await?;
        notices::notify_unfinished(&command, store.fail_unfinished(now).await?);
        let keys = open_keys(config_dir).await?;
        // A stopped server can leave the checkouts of its unfinished builds.
        let build_dir = config_dir.join(build::BUILD_DIR);
        build::remove_dir(&build_dir).await?;
        let pki = agent_pki(&config.platform, config_dir).await?;
        let agents = Arc::new(AgentRegistry::default());
        let public_url = resolve_public_url(config, &store).await?;
        Ok(Self {
            pki,
            forwarders: Arc::new(Forwarders::new(agents.clone())),
            agents,
            audit,
            store,
            keys: std::sync::RwLock::new(keys),
            local: Arc::new(local),
            config: config.platform.clone(),
            config_dir: config_dir.to_path_buf(),
            command,
            sent: Mutex::new(None),
            busy: std::sync::Mutex::new(HashSet::new()),
            queued: std::sync::Mutex::new(HashMap::new()),
            timing: deploy::Timing::default(),
            fetcher: Arc::new(GitFetcher),
            builds: Semaphore::new(build::MAX_BUILDS),
            build_dir,
            compose: Arc::new(DockerCompose::new(config.platform.docker.clone())),
            compose_dir: config_dir.join(compose::COMPOSE_DIR),
            backup_dir: config_dir.join(backup::BACKUP_DIR),
            http: tokio::sync::OnceCell::new(),
            pending: oauth::PendingAuthorizations::default(),
            renewing: Mutex::new(()),
            public_url: std::sync::RwLock::new(public_url),
            agent_upgrade_attempts: std::sync::Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn keys(&self) -> std::sync::RwLockReadGuard<'_, ClusterKeys> {
        match self.keys.read() {
            Ok(keys) => keys,
            Err(poisoned) => poisoned.into_inner(),
        }
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

    /// The runtime and the Compose runner of a target.
    fn backend(&self, target: ShortId) -> Backend {
        let compose: Arc<dyn ComposeRunner> = if is_local(target) {
            self.compose.clone()
        } else {
            Arc::new(RemoteCompose::new(
                target,
                self.agents.clone(),
                self.compose_dir.clone(),
            ))
        };
        Backend {
            runtime: self.runtime(target),
            compose,
        }
    }

    /// The runtime of a target: the local Docker Engine, or the agent of the target.
    fn runtime(&self, target: ShortId) -> Arc<dyn ContainerRuntime> {
        if is_local(target) {
            return self.local.clone();
        }
        Arc::new(RemoteRuntime::new(
            target,
            self.agents.clone(),
            self.forwarders.clone(),
        ))
    }

    pub async fn apps(&self) -> anyhow::Result<Vec<AppEntry>> {
        self.store.apps().await
    }

    pub async fn app(&self, id: ShortId) -> anyhow::Result<AppEntry> {
        self.store.app(id).await?.ok_or_else(|| not_found(id))
    }

    /// Adds an app. An app with a Git provider connection gets its webhook when the platform has
    /// a public address.
    pub async fn add_app(&self, request: AppRequest, now: u64) -> anyhow::Result<AppEntry> {
        let hook = self.check_request(&request).await?;
        let app = AppEntry {
            id: self.free_id().await?,
            name: request.name,
            target: request.target,
            spec: request.spec,
            git_token_set: false,
            webhook_secret_set: false,
            hook: None,
            update: None,
            catalog_update: None,
            created_at: now,
            updated_at: now,
        };
        written(self.store.insert_app(&app).await?, &app)?;
        self.sync_app_hook(app.id, hook, now).await?;
        self.app(app.id).await
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
        let hook = self.check_request(&request).await?;
        let current = self.app(id).await?;
        // The containers of the deployments run on the old target.
        if request.target != current.target && !self.store.deployments(id, 1).await?.is_empty() {
            return Err(Error::AppTargetFixed {
                name: current.name.to_string(),
            }
            .into());
        }
        let app = AppEntry {
            id,
            name: request.name,
            target: request.target,
            spec: request.spec,
            git_token_set: current.git_token_set,
            webhook_secret_set: current.webhook_secret_set,
            hook: None,
            update: current.update,
            catalog_update: None,
            created_at: current.created_at,
            updated_at: now,
        };
        written(self.store.update_app(&app).await?, &app)?;
        sync_running_limits(self, &app).await;
        self.sync_app_hook(id, hook, now).await?;
        self.app(id).await
    }

    /// Deletes an app with its containers, its network, its environment and its deployments.
    /// A database that another app uses cannot be deleted.
    pub async fn delete_app(self: &Arc<Self>, id: ShortId) -> anyhow::Result<AppEntry> {
        let app = self.app(id).await?;
        self.ensure_database_free(&app).await?;
        let _lock = self.lock_app(&app)?;
        self.cleanup_app_targets(&app, id).await?;
        self.drop_app_hook(id).await?;
        if !self.store.delete_app(id).await? {
            return Err(not_found(id));
        }
        self.forget_queued_hook(id);
        self.publish().await;
        Ok(app)
    }

    async fn cleanup_app_targets(&self, app: &AppEntry, id: ShortId) -> anyhow::Result<()> {
        if !self.store.deployments(id, 1).await?.is_empty() {
            for target in app.spec.targets_or_default(app.target) {
                self.remove_app_resources(&self.backend(target), id).await?;
            }
        }
        Ok(())
    }

    /// Checks for image updates of an app.
    pub async fn check_app_update(
        self: &Arc<Self>,
        id: ShortId,
    ) -> anyhow::Result<ImageUpdateStatus> {
        let status = image_check::check_app(self, id).await?;
        let app = self.app(id).await?;
        image_check::auto_deploy_if_needed(self, &app, &status).await?;
        Ok(status)
    }

    /// Refuses the deletion of a database that another app uses.
    async fn ensure_database_free(&self, app: &AppEntry) -> anyhow::Result<()> {
        if app.spec.database.is_none() {
            return Ok(());
        }
        let uses = self
            .store
            .apps()
            .await?
            .into_iter()
            .any(|other| other.spec.uses_database == Some(app.id));
        if uses {
            Err(Error::DatabaseInUse {
                name: app.name.to_string(),
            }
            .into())
        } else {
            Ok(())
        }
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

    /// Copies all environment variables from one app to another, re-sealing them with the target app's AAD.
    pub(super) async fn copy_app_env(&self, from: ShortId, to: ShortId) -> anyhow::Result<()> {
        let source_vars = self.store.env(from).await?;
        let mut new_vars = Vec::with_capacity(source_vars.len());
        for var in source_vars {
            let value = self.open_value(from, &var)?;
            let sealed = self.keys().seal(&env_aad(to, &var.key), value.as_bytes())?;
            new_vars.push(crate::platform::store::StoredEnv {
                key: var.key,
                sealed,
                secret: var.secret,
            });
        }
        self.store.replace_env(to, &new_vars).await
    }

    /// Lists all environment variable groups with summary information.
    pub async fn list_env_groups(&self) -> anyhow::Result<Vec<EnvGroupSummary>> {
        self.store.list_env_groups().await
    }

    /// The environment variable group and its variables (without secret values).
    pub async fn env_group(&self, id: ShortId) -> anyhow::Result<EnvGroup> {
        let stored_group = self
            .store
            .env_group(id)
            .await?
            .ok_or_else(|| not_found(id))?;
        let stored_vars = self.store.env_group_vars(id).await?;
        let variables = stored_vars
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
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(EnvGroup {
            id: stored_group.id,
            name: stored_group.name,
            description: stored_group.description,
            variables,
            created_at: stored_group.created_at,
            updated_at: stored_group.updated_at,
        })
    }

    /// Creates a new environment variable group.
    pub async fn create_env_group(&self, spec: EnvGroupSpec, now: u64) -> anyhow::Result<EnvGroup> {
        spec.validate()?;
        if self.store.env_group_by_name(&spec.name).await?.is_some() {
            return Err(Error::InvalidEnvGroup {
                reason: format!("group with name '{}' already exists", spec.name),
            }
            .into());
        }
        let id = self.free_env_group_id().await?;
        let empty_current = BTreeMap::new();
        let mut vars = Vec::with_capacity(spec.variables.len());
        for entry in spec.variables {
            vars.push(self.stored_env(id, entry, &empty_current)?);
        }
        self.store
            .insert_env_group(
                &StoredEnvGroup {
                    id,
                    name: spec.name,
                    description: spec.description,
                    created_at: now,
                    updated_at: now,
                },
                &vars,
            )
            .await?;
        self.env_group(id).await
    }

    /// Updates an environment variable group and replaces its variables.
    pub async fn update_env_group(
        &self,
        id: ShortId,
        spec: EnvGroupSpec,
        now: u64,
    ) -> anyhow::Result<EnvGroup> {
        spec.validate()?;
        let current = self
            .store
            .env_group(id)
            .await?
            .ok_or_else(|| not_found(id))?;
        self.check_group_name_unique(&spec.name, &current.name)
            .await?;
        let vars = self.prepare_updated_group_vars(id, spec.variables).await?;
        self.store
            .update_env_group(
                &StoredEnvGroup {
                    id,
                    name: spec.name,
                    description: spec.description,
                    created_at: current.created_at,
                    updated_at: now,
                },
                &vars,
            )
            .await?;
        self.env_group(id).await
    }

    async fn check_group_name_unique(
        &self,
        new_name: &str,
        current_name: &str,
    ) -> anyhow::Result<()> {
        if new_name != current_name && self.store.env_group_by_name(new_name).await?.is_some() {
            return Err(Error::InvalidEnvGroup {
                reason: format!("group with name '{new_name}' already exists"),
            }
            .into());
        }
        Ok(())
    }

    async fn prepare_updated_group_vars(
        &self,
        id: ShortId,
        variables: Vec<EnvEntry>,
    ) -> anyhow::Result<Vec<StoredEnv>> {
        let current_vars = self
            .store
            .env_group_vars(id)
            .await?
            .into_iter()
            .map(|var| (var.key.clone(), var))
            .collect::<BTreeMap<_, _>>();
        let mut vars = Vec::with_capacity(variables.len());
        for entry in variables {
            vars.push(self.stored_env(id, entry, &current_vars)?);
        }
        Ok(vars)
    }

    /// Deletes an environment variable group.
    pub async fn delete_env_group(&self, id: ShortId) -> anyhow::Result<EnvGroup> {
        let group = self.env_group(id).await?;
        let apps = self.store.apps().await?;
        let in_use = apps.iter().any(|app| app.spec.env_groups.contains(&id));
        if in_use {
            return Err(Error::InvalidEnvGroup {
                reason: "cannot delete environment variable group currently linked to apps".into(),
            }
            .into());
        }
        self.store.delete_env_group(id).await?;
        Ok(group)
    }

    async fn free_env_group_id(&self) -> anyhow::Result<ShortId> {
        for _ in 0..16 {
            let id = new_id()?;
            if self.store.env_group(id).await?.is_none() {
                return Ok(id);
            }
        }
        Err(anyhow::anyhow!("no free env group id"))
    }

    /// Sets the token that clones the private repository of an app, and returns the app.
    pub async fn set_git_token(&self, id: ShortId, token: &str) -> anyhow::Result<AppEntry> {
        self.app(id).await?;
        check_git_token(token)?;
        let sealed = self.keys().seal(&git_token_aad(id), token.as_bytes())?;
        self.store.set_secret(id, GIT_TOKEN, &sealed).await?;
        self.app(id).await
    }

    /// Deletes the Git token of an app, and returns the app.
    pub async fn delete_git_token(&self, id: ShortId) -> anyhow::Result<AppEntry> {
        self.app(id).await?;
        self.store.delete_secret(id, GIT_TOKEN).await?;
        self.app(id).await
    }

    /// The credential that clones the repository of an app: the current token of its Git
    /// provider connection, or its own Git token.
    async fn git_credential(
        &self,
        app: ShortId,
        connection: Option<ShortId>,
    ) -> anyhow::Result<Option<GitCredential>> {
        match connection {
            Some(connection) => Ok(Some(self.connection_credential(connection).await?)),
            None => Ok(self.git_token(app).await?.map(GitCredential::token)),
        }
    }

    /// The Git token of an app, opened for one clone.
    pub(crate) async fn git_token(&self, id: ShortId) -> anyhow::Result<Option<String>> {
        let Some(sealed) = self.store.secret(id, GIT_TOKEN).await? else {
            return Ok(None);
        };
        let token = self.keys().open(&git_token_aad(id), &sealed)?;
        Ok(Some(
            String::from_utf8(token).context("the Git token is not UTF-8")?,
        ))
    }

    /// The latest deployments of an app, the newest first.
    pub async fn deployments(&self, id: ShortId) -> anyhow::Result<Vec<DeploymentEntry>> {
        self.app(id).await?;
        self.store.deployments(id, DEPLOYMENT_LIST_LIMIT).await
    }

    /// The latest deployments across all apps, the newest first.
    pub async fn recent_deployments(&self, limit: u32) -> anyhow::Result<Vec<DeploymentEntry>> {
        self.store.recent_deployments(limit).await
    }

    /// The running deployment of every app.
    pub async fn running_deployments(&self) -> anyhow::Result<Vec<store::RunningDeployment>> {
        self.store.running_deployments().await
    }

    /// The last `tail` lines of the container log of the running deployment of an app. An app
    /// without a running deployment has an empty log.
    pub async fn app_log(
        &self,
        id: ShortId,
        tail: u32,
        service: Option<&str>,
    ) -> anyhow::Result<AppLog> {
        let app = self.app(id).await?;
        let Some(deployment) = self.store.running_deployment(id).await? else {
            return Ok(AppLog {
                log: String::new(),
                running: false,
            });
        };
        let name = self
            .resolve_container_name(&app, deployment, service)
            .await?;
        let runtime = self.runtime(app.target);
        let log = runtime.logs(&name, tail.clamp(1, MAX_LOG_TAIL)).await?;
        Ok(AppLog { log, running: true })
    }

    /// Streams the container logs of the running deployment of an app.
    pub async fn app_log_stream(
        &self,
        id: ShortId,
        tail: u32,
        service: Option<&str>,
    ) -> anyhow::Result<(bool, hyper::Response<hyper::body::Incoming>)> {
        let app = self.app(id).await?;
        let Some(deployment) = self.store.running_deployment(id).await? else {
            return Err(Error::AppNotRunning {
                name: app.name.to_string(),
            }
            .into());
        };
        let name = self
            .resolve_container_name(&app, deployment, service)
            .await?;
        let runtime = self.runtime(app.target);
        runtime
            .logs_stream(&name, tail.clamp(1, MAX_LOG_TAIL))
            .await
    }

    async fn resolve_container_name(
        &self,
        app: &AppEntry,
        deployment: ShortId,
        service: Option<&str>,
    ) -> anyhow::Result<ContainerName> {
        let Some(service) = service.filter(|s| !s.is_empty()) else {
            return proxy::container_name(app.id, deployment);
        };
        if !app.spec.source.is_compose() {
            return Err(Error::ServiceNotFound {
                service: service.to_string(),
            }
            .into());
        }
        let project = compose::project_name(app.id)?;
        let runtime = self.runtime(app.target);
        let containers = runtime.list_project_containers(&project).await?;
        find_service_container(&containers, service)
    }

    /// Returns the services and health of a Compose app.
    pub async fn app_services(&self, id: ShortId) -> anyhow::Result<Vec<ComposeServiceStatus>> {
        let app = self.app(id).await?;
        if !app.spec.source.is_compose() {
            return Ok(Vec::new());
        }
        let Some(_deployment) = self.store.running_deployment(id).await? else {
            return Ok(Vec::new());
        };
        let project = compose::project_name(id)?;
        let runtime = self.runtime(app.target);
        let containers = runtime.list_project_containers(&project).await?;
        let mut services = Vec::new();
        for c in containers {
            services.push(inspect_service_status(runtime.as_ref(), c).await);
        }
        services.sort_by(|a, b| a.service.cmp(&b.service));
        Ok(services)
    }

    /// Opens a terminal into the container of the running deployment of an app. Only the local
    /// target carries a terminal; an agent target answers `terminal_unsupported`.
    pub async fn app_terminal(
        &self,
        id: ShortId,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<(String, ExecHandle)> {
        let app = self.app(id).await?;
        if !is_local(app.target) {
            return Err(Error::TerminalUnsupported.into());
        }
        let Some(deployment) = self.store.running_deployment(id).await? else {
            return Err(Error::AppNotRunning {
                name: app.name.to_string(),
            }
            .into());
        };
        let name = proxy::container_name(id, deployment)?;
        let exec = self.local.exec(&name, cols, rows).await?;
        Ok((name.to_string(), exec))
    }

    /// The files of the running deployment of an app, over the archive API. Only the local
    /// target carries them; an agent target answers `terminal_unsupported`.
    async fn file_runtime(
        &self,
        id: ShortId,
    ) -> anyhow::Result<(AppEntry, ContainerName, Arc<dyn ContainerRuntime>)> {
        let app = self.app(id).await?;
        if !is_local(app.target) {
            return Err(Error::TerminalUnsupported.into());
        }
        let Some(deployment) = self.store.running_deployment(id).await? else {
            return Err(Error::AppNotRunning {
                name: app.name.to_string(),
            }
            .into());
        };
        let name = proxy::container_name(id, deployment)?;
        let runtime = self.runtime(app.target);
        Ok((app, name, runtime))
    }

    /// The entries of a directory inside the container of the running deployment.
    pub async fn list_files(&self, id: ShortId, path: &str) -> anyhow::Result<Vec<FileEntry>> {
        let (_, name, runtime) = self.file_runtime(id).await?;
        runtime.list_dir(&name, path).await
    }

    /// The content of a file inside the container of the running deployment.
    pub async fn download_file(&self, id: ShortId, path: &str) -> anyhow::Result<Vec<u8>> {
        let (_, name, runtime) = self.file_runtime(id).await?;
        runtime.download_file(&name, path).await
    }

    /// Writes a file inside the container of the running deployment.
    pub async fn upload_file(&self, id: ShortId, path: &str, bytes: Vec<u8>) -> anyhow::Result<()> {
        let (_, name, runtime) = self.file_runtime(id).await?;
        runtime.upload_file(&name, path, bytes).await
    }

    /// Removes a file or a directory inside the container of the running deployment.
    pub async fn delete_path(&self, id: ShortId, path: &str) -> anyhow::Result<()> {
        let (_, name, runtime) = self.file_runtime(id).await?;
        runtime.remove_path(&name, path).await
    }

    pub async fn deployment(&self, id: ShortId) -> anyhow::Result<DeploymentEntry> {
        self.store
            .deployment(id)
            .await?
            .ok_or_else(|| not_found(id))
    }

    /// Checks a request and returns the webhook target of its connection.
    async fn check_request(
        &self,
        request: &AppRequest,
    ) -> anyhow::Result<Option<app_hooks::HookTarget>> {
        request.spec.validate()?;
        if let Some(port) = request.spec.host_port
            && !host_port_free(port)
        {
            return Err(Error::HostPortUnavailable { port }.into());
        }
        if !self.store.has_target(request.target).await? {
            return Err(not_found(request.target));
        }
        for group_id in &request.spec.env_groups {
            if self.store.env_group(*group_id).await?.is_none() {
                return Err(not_found(*group_id));
            }
        }
        self.hook_target(&request.spec).await
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
                self.keys().seal(&env_aad(id, &key), value.as_bytes())?
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

    pub(crate) fn open_value(&self, id: ShortId, var: &StoredEnv) -> anyhow::Result<String> {
        let value = self.keys().open(&env_aad(id, &var.key), &var.sealed)?;
        String::from_utf8(value).context("an environment value is not UTF-8")
    }

    /// Rotates the platform encryption key: generates a new primary key, moves the previous
    /// primary to older keys, re-seals all stored secrets in SQLite, and persists the new key.
    pub async fn rotate_platform_key(
        &self,
    ) -> anyhow::Result<r3v3rs3_api::platform::KeyRotationReport> {
        rotate::run_key_rotation(self.store.pool(), &self.config_dir, &self.keys).await
    }
}

/// The associated data of a sealed value binds it to its app and its key, so a value copied to
/// another row does not open.
pub(super) fn env_aad(app: ShortId, key: &str) -> String {
    format!("app/{app}/env/{key}")
}

/// Whether nothing listens on the fixed host port of `127.0.0.1` right now. The check is
/// best effort: between it and the deployment another process can take the port, and the
/// deployment fails with the reason of Docker.
fn host_port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// A free port of `127.0.0.1` for a fixed host port, chosen by the kernel.
pub fn allocate_host_port() -> anyhow::Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

pub(super) fn git_token_aad(app: ShortId) -> String {
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

/// Whether a target is the Docker Engine of the server itself.
fn is_local(target: ShortId) -> bool {
    target.to_string() == LOCAL_TARGET
}

fn not_found(id: ShortId) -> anyhow::Error {
    Error::IdNotFound { id: id.to_string() }.into()
}

/// The result of a write of an app.
fn written(write: Write, app: &AppEntry) -> anyhow::Result<()> {
    match write {
        Write::Done => Ok(()),
        Write::NameTaken => Err(name_taken(app).into()),
        Write::NotFound => Err(not_found(app.id)),
    }
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
    let mut paths = vec![key_path];
    let old_key_path = config_dir.join("platform.key.old");
    if tokio::fs::try_exists(&old_key_path).await? {
        paths.push(old_key_path);
    }
    load_keys(&paths).await
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
pub fn docker_runtime(endpoint: &str) -> anyhow::Result<DockerRuntime> {
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

fn find_service_container(
    containers: &[ContainerSummary],
    service: &str,
) -> anyhow::Result<ContainerName> {
    for c in containers {
        let matches_service =
            c.labels.get(COMPOSE_SERVICE_LABEL).map(String::as_str) == Some(service);
        if matches_service || c.name == service {
            return Ok(c.name.parse()?);
        }
    }
    Err(Error::ServiceNotFound {
        service: service.to_string(),
    }
    .into())
}

async fn inspect_service_status(
    runtime: &dyn ContainerRuntime,
    summary: ContainerSummary,
) -> ComposeServiceStatus {
    let service = summary
        .labels
        .get(COMPOSE_SERVICE_LABEL)
        .cloned()
        .unwrap_or_else(|| summary.name.clone());
    let (state, health, exit_code) = match summary.name.parse::<ContainerName>() {
        Ok(name) => match runtime.inspect_container(&name).await {
            Ok(Some(info)) => (
                if info.running {
                    "running".to_string()
                } else {
                    "exited".to_string()
                },
                info.health,
                if !info.running {
                    Some(info.exit_code)
                } else {
                    None
                },
            ),
            _ => (summary.state, None, None),
        },
        _ => (summary.state, None, None),
    };
    ComposeServiceStatus {
        service,
        container: summary.name,
        state,
        health,
        exit_code,
    }
}

/// Dynamically syncs updated resource limits to the running container, if one exists.
async fn sync_running_limits(platform: &Platform, app: &AppEntry) {
    let Ok(Some(deployment)) = platform.store.running_deployment(app.id).await else {
        return;
    };
    let Ok(container) = proxy::container_name(app.id, deployment) else {
        return;
    };
    let runtime = platform.runtime(app.target);
    let _ = runtime
        .update_container_limits(&container, &app.spec.limits)
        .await;
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
        let mut platform = Platform::open(&config, &dir.0, command, None).await?;
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
                ports: Vec::new(),
                health_check_path: None,
                volumes: Vec::new(),
                restart: Default::default(),
                limits: Default::default(),
                host_port: None,
                database: None,
                uses_database: None,
                replica_of: None,
                auto_deploy: false,
                require_approval: false,
                deploy_window: None,
                tag_trigger: None,
                store_app_id: None,
                targets: Vec::new(),
                load_balancing: None,
                alert_rules: None,
                sticky: None,
                git_config: None,
                env_groups: Vec::new(),
                preview: None,
                preview_of: None,
                buildkit: None,
                bot_blocker: false,
                grpc_web: false,
                project_id: None,
                environment_id: None,
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

    pub(super) fn api_error(err: anyhow::Error) -> Error {
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
        let audit = Arc::new(AuditLog::new(
            Arc::new(crate::audit::SqliteAuditStore::new(
                dir.join("r3v3rs3-unused-audit.db"),
            )),
            String::new(),
        ));
        let mut config = AppConfig::default();
        let handle =
            PlatformHandle::start(&config, &dir, command.clone(), &events, audit.clone()).await;
        assert!(matches!(handle.get(), Err(Error::PlatformDisabled)));

        config.platform.enabled = true;
        config.platform.docker = "https://docker.example:2376".into();
        let handle = PlatformHandle::start(&config, &dir, command, &events, audit).await;
        assert!(matches!(handle.get(), Err(Error::PlatformFailed)));
    }

    #[tokio::test]
    async fn env_groups_crud_and_app_linking() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;

        // Create an env group
        let spec = EnvGroupSpec {
            name: "Shared Config".to_string(),
            description: Some("Common configuration".to_string()),
            variables: vec![
                env("APP_REGION", Some("eu-west-1"), false),
                env("SHARED_SECRET", Some("super-secret-123"), true),
            ],
        };
        let group = platform.create_env_group(spec, 1000).await?;
        assert_eq!(group.name, "Shared Config");
        assert_eq!(group.variables.len(), 2);

        // List groups
        let list = platform.list_env_groups().await?;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Shared Config");
        assert_eq!(list[0].variable_count, 2);

        // Fetch group
        let fetched = platform.env_group(group.id).await?;
        assert_eq!(fetched.name, "Shared Config");
        let region = fetched
            .variables
            .iter()
            .find(|v| v.key.as_str() == "APP_REGION")
            .unwrap();
        assert_eq!(region.value.as_deref(), Some("eu-west-1"));
        let secret = fetched
            .variables
            .iter()
            .find(|v| v.key.as_str() == "SHARED_SECRET")
            .unwrap();
        assert_eq!(secret.value, None);
        assert!(secret.secret);

        // Update group keeping secret
        let update_spec = EnvGroupSpec {
            name: "Shared Config Renamed".to_string(),
            description: None,
            variables: vec![
                env("APP_REGION", Some("us-east-1"), false),
                env("SHARED_SECRET", None, true),
                env("EXTRA_FLAG", Some("true"), false),
            ],
        };
        let updated = platform
            .update_env_group(group.id, update_spec, 2000)
            .await?;
        assert_eq!(updated.name, "Shared Config Renamed");
        assert_eq!(updated.variables.len(), 3);

        // Link to app
        let mut app_req = request("my-app");
        app_req.spec.env_groups = vec![group.id];
        let app = platform.add_app(app_req, 3000).await?;
        assert_eq!(app.spec.env_groups, vec![group.id]);

        // Attempting to delete linked group fails
        let del_err = platform.delete_env_group(group.id).await.unwrap_err();
        assert!(matches!(api_error(del_err), Error::InvalidEnvGroup { .. }));

        // Unlink from app
        let mut updated_spec = app.spec.clone();
        updated_spec.env_groups.clear();
        platform
            .update_app(
                app.id,
                AppRequest {
                    name: app.name,
                    target: app.target,
                    spec: updated_spec,
                },
                4000,
            )
            .await?;

        // Now delete succeeds
        let deleted = platform.delete_env_group(group.id).await?;
        assert_eq!(deleted.id, group.id);
        assert!(platform.list_env_groups().await?.is_empty());
        Ok(())
    }

    async fn setup_test_project_and_env(platform: &Platform) -> anyhow::Result<(ShortId, ShortId)> {
        let spec = r3v3rs3_api::platform::ProjectSpec {
            name: "Shop Project".to_string(),
            description: Some("Storefront".to_string()),
            default_environments: vec!["production".to_string(), "staging".to_string()],
        };
        let project = platform.create_project(spec, 1000).await?;
        let prod_env_id = project.environments[0].id;
        platform
            .set_environment_env(
                prod_env_id,
                r3v3rs3_api::platform::EnvironmentEnvSpec {
                    variables: vec![
                        env("API_URL", Some("https://api.shop.com"), false),
                        env("API_SECRET", Some("super-secret-key"), true),
                    ],
                },
            )
            .await?;
        Ok((project.id, prod_env_id))
    }

    #[tokio::test]
    async fn projects_and_environments_crud_and_app_linking() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let (project_id, prod_env_id) = setup_test_project_and_env(&platform).await?;

        let list = platform.list_projects().await?;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Shop Project");
        assert_eq!(list[0].environments_count, 2);

        let fetched_env = platform.environment(prod_env_id).await?;
        assert_eq!(fetched_env.variables.len(), 2);
        let secret = fetched_env
            .variables
            .iter()
            .find(|v| v.key.as_str() == "API_SECRET")
            .unwrap();
        assert_eq!(secret.value, None);
        assert!(secret.secret);

        let mut app_req = request("shop-frontend");
        app_req.spec.project_id = Some(project_id);
        app_req.spec.environment_id = Some(prod_env_id);
        let app = platform.add_app(app_req, 2000).await?;
        assert_eq!(app.spec.project_id, Some(project_id));
        assert_eq!(app.spec.environment_id, Some(prod_env_id));

        let proj_details = platform.project(project_id).await?;
        let prod_apps = proj_details
            .environments
            .iter()
            .find(|e| e.id == prod_env_id)
            .unwrap()
            .apps_count;
        assert_eq!(prod_apps, 1);

        let del_err = platform.delete_project(project_id).await.unwrap_err();
        assert!(matches!(api_error(del_err), Error::InvalidProject { .. }));

        platform.delete_app(app.id).await?;

        let deleted = platform.delete_project(project_id).await?;
        assert_eq!(deleted.id, project_id);
        assert!(platform.list_projects().await?.is_empty());
        Ok(())
    }

    #[test]
    fn test_find_service_container() {
        let mut labels = BTreeMap::new();
        labels.insert(COMPOSE_SERVICE_LABEL.to_string(), "web".to_string());
        let containers = vec![
            ContainerSummary {
                id: "c1".to_string(),
                name: "project-web-1".to_string(),
                state: "running".to_string(),
                labels,
            },
            ContainerSummary {
                id: "c2".to_string(),
                name: "redis".to_string(),
                state: "running".to_string(),
                labels: BTreeMap::new(),
            },
        ];
        let found = find_service_container(&containers, "web").unwrap();
        assert_eq!(found.as_str(), "project-web-1");

        let found_by_name = find_service_container(&containers, "redis").unwrap();
        assert_eq!(found_by_name.as_str(), "redis");

        let missing = find_service_container(&containers, "db");
        assert!(missing.is_err());
    }

    #[tokio::test]
    async fn non_compose_app_has_no_services() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let app = platform.add_app(request("my-app"), 1000).await?;
        let services = platform.app_services(app.id).await?;
        assert!(services.is_empty());
        Ok(())
    }
}
