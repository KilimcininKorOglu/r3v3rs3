//! The deployment platform: the apps, their targets and their environment variables.
//!
//! The admin API calls the platform directly instead of through the server loop, because a
//! database query or a Docker call must not hold up the ports and the other RPC methods.

pub mod proxy;
mod publish;
pub mod store;

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
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppRequest, DeploymentEntry, EnvEntry, PlatformConfig, TargetEntry,
};
use rand::seq::IndexedRandom;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use store::{PlatformStore, StoredEnv, Write};
use tokio::sync::{Mutex, mpsc};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tracing::error;

/// The database of the platform in the config directory.
pub const DATABASE_FILE: &str = "platform.db";

/// The encryption key of the environment variables in the config directory.
pub const KEY_FILE: &str = "platform.key";

/// The most deployments that one list returns.
pub const DEPLOYMENT_LIST_LIMIT: u32 = 100;

/// The longest value of an environment variable in bytes.
const MAX_ENV_VALUE_LENGTH: usize = 64 * 1024;

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
        let key_path = config_dir.join(KEY_FILE);
        if !tokio::fs::try_exists(&key_path).await? {
            write_new_key_file(&key_path).await?;
        }
        let keys = load_keys(std::slice::from_ref(&key_path)).await?;
        Ok(Self {
            store,
            keys,
            local: Arc::new(local),
            config: config.platform.clone(),
            command,
            sent: Mutex::new(None),
        })
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
            created_at: current.created_at,
            updated_at: now,
        };
        match self.store.update_app(&app).await? {
            Write::Done => Ok(app),
            Write::NameTaken => Err(name_taken(&app).into()),
            Write::NotFound => Err(not_found(id)),
        }
    }

    pub async fn delete_app(&self, id: ShortId) -> anyhow::Result<AppEntry> {
        let app = self.app(id).await?;
        if !self.store.delete_app(id).await? {
            return Err(not_found(id));
        }
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

    /// The latest deployments of an app, the newest first.
    pub async fn deployments(&self, id: ShortId) -> anyhow::Result<Vec<DeploymentEntry>> {
        self.app(id).await?;
        self.store.deployments(id, DEPLOYMENT_LIST_LIMIT).await
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

    pub(super) struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn platform() -> anyhow::Result<(Platform, TempDir)> {
        // These tests read no commands.
        let (command, _) = mpsc::channel(1);
        platform_with(&PlatformConfig::default(), command).await
    }

    pub(super) async fn platform_with(
        platform: &PlatformConfig,
        command: mpsc::Sender<ServerCommand>,
    ) -> anyhow::Result<(Platform, TempDir)> {
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
        Ok((Platform::open(&config, &dir.0, command).await?, dir))
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
        let mut config = AppConfig::default();
        let handle = PlatformHandle::start(&config, &dir, command.clone()).await;
        assert!(matches!(handle.get(), Err(Error::PlatformDisabled)));

        config.platform.enabled = true;
        config.cluster.enabled = true;
        let handle = PlatformHandle::start(&config, &dir, command.clone()).await;
        assert!(matches!(handle.get(), Err(Error::PlatformInCluster)));

        config.cluster.enabled = false;
        config.platform.docker = "https://docker.example:2376".into();
        let handle = PlatformHandle::start(&config, &dir, command).await;
        assert!(matches!(handle.get(), Err(Error::PlatformFailed)));
    }
}
