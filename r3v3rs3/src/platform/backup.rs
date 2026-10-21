//! The backups of the managed databases. A backup is the dump of the engine, gzipped, in the
//! `backups` directory of the config directory, with one JSON manifest per file. A restore
//! uploads the dump back into the running container and runs the restore command of the engine.

use super::{Platform, allocate_host_port, is_local, proxy};
use crate::clock::unix_ms;
use crate::runtime::ContainerRuntime;
use anyhow::{Context, bail};
use async_compression::tokio::bufread::GzipEncoder;
use r3v3rs3_api::container::{AppName, ContainerName};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, BackupConfig, BackupEntry, EngineInfo, PitrRange, PitrRestoreRequest,
    PitrRestoreResponse, RestoreFromS3Request, RestoreFromS3Response, S3BackupSnapshot, WalSegment,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::io::AsyncReadExt;

/// The settings row of the backup configuration in the platform database.
pub const BACKUP_CONFIG: &str = "backup_config";

/// The directory under the config directory that holds the backups.
pub const BACKUP_DIR: &str = "backups";

/// The settings row of the sealed S3 secret key.
pub(super) const S3_SECRET: &str = "backup_s3_secret";

/// The associated data that binds the sealed S3 secret key.
pub(super) const S3_SECRET_AAD: &str = "backup/s3/secret_key";

/// The settings row of the sealed SFTP secret key/password.
pub(super) const SFTP_SECRET: &str = "backup_sftp_secret";

/// The associated data that binds the sealed SFTP secret.
pub(super) const SFTP_SECRET_AAD: &str = "backup/sftp/secret";

/// The settings row of the sealed rsync secret password.
pub(super) const RSYNC_SECRET: &str = "backup_rsync_secret";

/// The associated data that binds the sealed rsync secret.
pub(super) const RSYNC_SECRET_AAD: &str = "backup/rsync/secret";

/// The directory inside the container that a restore uploads its dump to.
const RESTORE_DIR: &str = "/tmp/r3v3rs3-restore";

/// How the engine takes its backup: one command over `exec`, or one file to download.
struct DumpPlan {
    command: Vec<String>,
    env: Vec<String>,
    /// The file that carries the dump, for an engine that cannot write it to stdout.
    file: Option<String>,
}

/// How the engine takes its dump back: the file that the restore uploads, and the command that
/// reads it.
struct RestorePlan {
    /// The target path of the upload. `None` for an engine that takes the file in place.
    upload: Option<String>,
    command: Vec<String>,
    env: Vec<String>,
}

impl Platform {
    /// The backup settings stored in the platform database, or the defaults.
    pub async fn backup_config(&self) -> anyhow::Result<BackupConfig> {
        match self.store.setting(BACKUP_CONFIG).await? {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(BackupConfig::default()),
        }
    }

    /// Replaces the backup settings. Secret keys/passwords in the request replace the stored ones,
    /// empty ones keep them, and removing a target clears its secret. The settings are stored
    /// without secrets; they live sealed in their own rows.
    pub async fn set_backup_config(&self, mut config: BackupConfig) -> anyhow::Result<()> {
        self.save_s3_secret(&mut config.s3).await?;
        self.save_sftp_secret(&mut config.sftp).await?;
        self.save_rsync_secret(&mut config.rsync).await?;
        let json = serde_json::to_string(&config)?;
        self.store.set_setting(BACKUP_CONFIG, Some(&json)).await
    }

    async fn save_s3_secret(
        &self,
        s3: &mut Option<r3v3rs3_api::platform::S3Config>,
    ) -> anyhow::Result<()> {
        let secret = s3
            .as_mut()
            .map(|s| std::mem::take(&mut s.secret_key))
            .unwrap_or_default();
        match (s3.is_some(), secret.is_empty()) {
            (true, false) => {
                let sealed = hex::encode(self.keys().seal(S3_SECRET_AAD, secret.as_bytes())?);
                self.store.set_setting(S3_SECRET, Some(&sealed)).await
            }
            (true, true) => Ok(()),
            (false, _) => self.store.set_setting(S3_SECRET, None).await,
        }
    }

    async fn save_sftp_secret(
        &self,
        sftp: &mut Option<r3v3rs3_api::platform::SftpConfig>,
    ) -> anyhow::Result<()> {
        let secret = sftp
            .as_mut()
            .map(|s| {
                if !s.key.is_empty() {
                    std::mem::take(&mut s.key)
                } else {
                    std::mem::take(&mut s.password)
                }
            })
            .unwrap_or_default();
        match (sftp.is_some(), secret.is_empty()) {
            (true, false) => {
                let sealed = hex::encode(self.keys().seal(SFTP_SECRET_AAD, secret.as_bytes())?);
                self.store.set_setting(SFTP_SECRET, Some(&sealed)).await
            }
            (true, true) => Ok(()),
            (false, _) => self.store.set_setting(SFTP_SECRET, None).await,
        }
    }

    async fn save_rsync_secret(
        &self,
        rsync: &mut Option<r3v3rs3_api::platform::RsyncConfig>,
    ) -> anyhow::Result<()> {
        let secret = rsync
            .as_mut()
            .map(|r| std::mem::take(&mut r.password))
            .unwrap_or_default();
        match (rsync.is_some(), secret.is_empty()) {
            (true, false) => {
                let sealed = hex::encode(self.keys().seal(RSYNC_SECRET_AAD, secret.as_bytes())?);
                self.store.set_setting(RSYNC_SECRET, Some(&sealed)).await
            }
            (true, true) => Ok(()),
            (false, _) => self.store.set_setting(RSYNC_SECRET, None).await,
        }
    }

    /// The S3 secret key, unsealed. `None` when no secret is stored.
    pub async fn s3_secret(&self) -> anyhow::Result<Option<String>> {
        let Some(sealed) = self.store.setting(S3_SECRET).await? else {
            return Ok(None);
        };
        let sealed_bytes = hex::decode(sealed)?;
        let bytes = self.keys().open(S3_SECRET_AAD, &sealed_bytes)?;
        Ok(Some(String::from_utf8(bytes)?))
    }

    /// The SFTP password or private key, unsealed. `None` when no secret is stored.
    pub async fn sftp_secret(&self) -> anyhow::Result<Option<String>> {
        let Some(sealed) = self.store.setting(SFTP_SECRET).await? else {
            return Ok(None);
        };
        let sealed_bytes = hex::decode(sealed)?;
        let bytes = self.keys().open(SFTP_SECRET_AAD, &sealed_bytes)?;
        Ok(Some(String::from_utf8(bytes)?))
    }

    /// The rsync password, unsealed. `None` when no secret is stored.
    pub async fn rsync_secret(&self) -> anyhow::Result<Option<String>> {
        let Some(sealed) = self.store.setting(RSYNC_SECRET).await? else {
            return Ok(None);
        };
        let sealed_bytes = hex::decode(sealed)?;
        let bytes = self.keys().open(RSYNC_SECRET_AAD, &sealed_bytes)?;
        Ok(Some(String::from_utf8(bytes)?))
    }

    /// The backups of one database, newest first.
    pub async fn list_backups(&self, db_id: ShortId) -> anyhow::Result<Vec<BackupEntry>> {
        let dir = self.backup_dir(db_id).await?;
        let mut entries = Vec::new();
        let mut reader = match tokio::fs::read_dir(&dir).await {
            Ok(reader) => reader,
            // A database without a backup has no directory yet.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
            Err(err) => return Err(err.into()),
        };
        while let Some(file) = reader.next_entry().await? {
            let path = file.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && let Ok(json) = tokio::fs::read(&path).await
                && let Ok(entry) = serde_json::from_slice::<BackupEntry>(&json)
            {
                entries.push(entry);
            }
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.created_at));
        Ok(entries)
    }

    /// Dumps the database of the local target into the backup directory, and applies the
    /// retention of the settings.
    pub async fn run_backup(&self, db_id: ShortId) -> anyhow::Result<BackupEntry> {
        let (app, container, runtime) = self.database_container(db_id).await?;
        let engine = app
            .spec
            .database
            .clone()
            .context("the app is not a managed database")?;
        let info = r3v3rs3_api::platform::engine_info(&engine)
            .context("the engine is not in the database catalog")?;
        let password = self.database_password(db_id, &info).await?;
        let dump = take_dump(&runtime, &container, dump_plan(&engine, &info, &password)?).await?;
        let compressed = gzip(&dump).await?;
        let entry = BackupEntry {
            id: backup_id(),
            app: db_id,
            name: app.name.to_string(),
            engine,
            size: compressed.len() as u64,
            sha256: hex::encode(Sha256::digest(&compressed)),
            created_at: unix_ms() as i64,
            verified_at: None,
            verification_status: None,
        };
        self.store_backup(db_id, &entry, compressed).await?;
        self.apply_retention(db_id).await?;
        Ok(entry)
    }

    /// Writes one backup file with its manifest, uploads it to remote targets, and runs initial
    /// restore testing when enabled.
    async fn store_backup(
        &self,
        db_id: ShortId,
        entry: &BackupEntry,
        compressed: Vec<u8>,
    ) -> anyhow::Result<()> {
        let dir = self.backup_dir(db_id).await?;
        tokio::fs::create_dir_all(&dir).await?;
        let dump_path = dir.join(format!("{}.dump.gz", entry.id));
        let manifest_path = dir.join(format!("{}.json", entry.id));
        tokio::fs::write(&dump_path, &compressed).await?;
        let manifest = serde_json::to_vec(entry)?;
        tokio::fs::write(&manifest_path, manifest).await?;

        self.dispatch_remote_backups(entry, &compressed, &dump_path, &manifest_path)
            .await;

        let config = self.backup_config().await.unwrap_or_default();
        if config.restore_test_enabled {
            let _ = self.verify_backup(db_id, &entry.id).await;
        }
        Ok(())
    }

    async fn dispatch_remote_backups(
        &self,
        entry: &BackupEntry,
        compressed: &[u8],
        dump_path: &std::path::Path,
        manifest_path: &std::path::Path,
    ) {
        if let Err(err) = self.upload_to_s3(entry, compressed.to_vec()).await {
            tracing::error!(
                err = format!("{err:#}"),
                "failed to upload the backup to S3"
            );
        }
        if let Err(err) = self.upload_to_sftp(entry, dump_path, manifest_path).await {
            tracing::error!(
                err = format!("{err:#}"),
                "failed to upload the backup to SFTP"
            );
        }
        if let Err(err) = self.upload_to_rsync(entry, dump_path, manifest_path).await {
            tracing::error!(
                err = format!("{err:#}"),
                "failed to sync the backup with rsync"
            );
        }
    }

    /// Removes one backup and its manifest.
    pub async fn delete_backup(&self, db_id: ShortId, backup_id: &str) -> anyhow::Result<()> {
        let entry = self.backup_entry(db_id, backup_id).await?;
        let dir = self.backup_dir(db_id).await?;
        tokio::fs::remove_file(dir.join(format!("{}.dump.gz", entry.id))).await?;
        tokio::fs::remove_file(dir.join(format!("{}.json", entry.id))).await?;
        Ok(())
    }

    /// The content of one backup.
    pub async fn download_backup(
        &self,
        db_id: ShortId,
        backup_id: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let entry = self.backup_entry(db_id, backup_id).await?;
        let dir = self.backup_dir(db_id).await?;
        Ok(tokio::fs::read(dir.join(format!("{}.dump.gz", entry.id))).await?)
    }

    /// Uploads a stored backup into the running container of the database and runs the restore
    /// command of the engine. Redis replaces its dump file and restarts.
    pub async fn restore_backup(&self, db_id: ShortId, backup_id: &str) -> anyhow::Result<()> {
        let (app, container, runtime) = self.database_container(db_id).await?;
        let engine = app
            .spec
            .database
            .clone()
            .context("the app is not a managed database")?;
        let info = r3v3rs3_api::platform::engine_info(&engine)
            .context("the engine is not in the database catalog")?;
        let password = self.database_password(db_id, &info).await?;
        let plan = restore_plan(&engine, &info, &password)?;
        let bytes = ungzip(&self.download_backup(db_id, backup_id).await?).await?;
        run_restore(&runtime, &container, &plan, bytes, &info.data_path)
            .await
            .map_err(|err| Error::RestoreFailed {
                reason: format!("{err:#}"),
            })?;
        Ok(())
    }

    /// Extracts the dump bytes of a source database, either from its running container or from
    /// its latest backup.
    async fn get_clone_dump(
        &self,
        source_id: ShortId,
        engine: &str,
        info: &EngineInfo,
    ) -> anyhow::Result<Vec<u8>> {
        if let Ok((_app, container, runtime)) = self.database_container(source_id).await {
            let password = self.database_password(source_id, info).await?;
            let plan = dump_plan(engine, info, &password)?;
            if let Ok(dump) = take_dump(&runtime, &container, plan).await {
                return Ok(dump);
            }
        }
        let backups = self.list_backups(source_id).await?;
        if let Some(latest) = backups.first() {
            let compressed = self.download_backup(source_id, &latest.id).await?;
            return ungzip(&compressed).await;
        }
        bail!("source database is not running and has no backups to clone from");
    }

    /// Waits for a cloned database container to be ready and restores the dump bytes into it.
    async fn wait_and_restore(
        &self,
        db_id: ShortId,
        plan: &RestorePlan,
        dump: Vec<u8>,
        data_path: &str,
    ) -> anyhow::Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            if let Ok((_app, container, runtime)) = self.database_container(db_id).await
                && run_restore(&runtime, &container, plan, dump.clone(), data_path)
                    .await
                    .is_ok()
            {
                tracing::info!(%db_id, "database clone restored successfully");
                return Ok(());
            }
        }
        bail!("timed out waiting for cloned database container to be ready for restore");
    }

    /// Copies data from a source database into a newly deployed cloned database.
    pub async fn restore_clone_data(
        self: &Arc<Self>,
        source_id: ShortId,
        dest_id: ShortId,
        dest_password: &str,
    ) -> anyhow::Result<()> {
        let source = self.app(source_id).await?;
        let engine = source
            .spec
            .database
            .clone()
            .context("the source app is not a managed database")?;
        let info = r3v3rs3_api::platform::engine_info(&engine)
            .context("the engine is not in the database catalog")?;

        let dump_bytes = self.get_clone_dump(source_id, &engine, &info).await?;
        let restore = restore_plan(&engine, &info, dest_password)?;
        let platform = self.clone();
        let data_path = info.data_path.clone();

        tokio::spawn(async move {
            if let Err(err) = platform
                .wait_and_restore(dest_id, &restore, dump_bytes, &data_path)
                .await
            {
                tracing::error!(%err, app = %dest_id, "failed to restore cloned data into new database");
            }
        });
        Ok(())
    }

    /// Lists database backup snapshots stored in the configured S3 storage.
    pub async fn list_s3_backups(
        &self,
        db_filter: Option<&str>,
    ) -> anyhow::Result<Vec<S3BackupSnapshot>> {
        let config = self.backup_config().await?;
        let Some(s3) = config.s3 else {
            return Err(Error::S3NotConfigured.into());
        };
        let Some(secret) = self.s3_secret().await? else {
            return Err(Error::S3NotConfigured.into());
        };
        let s3 = r3v3rs3_api::platform::S3Config {
            secret_key: secret,
            ..s3
        };
        let http_client = self.http().await?;
        let filter = db_filter.unwrap_or_default();
        let items = super::s3::list_objects(http_client, &s3, filter)
            .await
            .map_err(|err| Error::S3OperationFailed {
                reason: err.to_string(),
            })?;
        let mut snapshots = Vec::new();
        for item in items {
            if let Some(snap) =
                parse_backup_snapshot(&s3.prefix, &item.key, item.size, &item.last_modified)
            {
                if let Some(f) = db_filter
                    && !snap.database_name.eq_ignore_ascii_case(f)
                {
                    continue;
                }
                snapshots.push(snap);
            }
        }
        snapshots.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
        Ok(snapshots)
    }

    /// Downloads the content of one S3 backup snapshot.
    pub async fn download_s3_backup(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let config = self.backup_config().await?;
        let Some(s3) = config.s3 else {
            return Err(Error::S3NotConfigured.into());
        };
        let Some(secret) = self.s3_secret().await? else {
            return Err(Error::S3NotConfigured.into());
        };
        let s3 = r3v3rs3_api::platform::S3Config {
            secret_key: secret,
            ..s3
        };
        let http_client = self.http().await?;
        super::s3::download_key(http_client, &s3, key)
            .await
            .map_err(|err| {
                Error::S3OperationFailed {
                    reason: err.to_string(),
                }
                .into()
            })
    }

    /// Restores a database from an S3 remote backup snapshot.
    pub async fn restore_s3_backup(
        self: &Arc<Self>,
        req: RestoreFromS3Request,
        caller: &str,
    ) -> anyhow::Result<RestoreFromS3Response> {
        let raw_bytes = self.download_s3_backup(&req.key).await?;
        let dump_bytes = if is_gzipped(&raw_bytes) {
            ungzip(&raw_bytes).await?
        } else {
            raw_bytes
        };
        if let Some(target_id) = req.target_db_id {
            self.restore_s3_into_existing(target_id, dump_bytes).await
        } else {
            self.restore_s3_as_new(req, caller, dump_bytes).await
        }
    }

    async fn restore_s3_into_existing(
        &self,
        target_id: ShortId,
        dump_bytes: Vec<u8>,
    ) -> anyhow::Result<RestoreFromS3Response> {
        let (app, container, runtime) = self.database_container(target_id).await?;
        let engine = app
            .spec
            .database
            .clone()
            .context("the target app is not a managed database")?;
        let info = r3v3rs3_api::platform::engine_info(&engine)
            .context("the engine is not in the database catalog")?;
        let password = self.database_password(target_id, &info).await?;
        let plan = restore_plan(&engine, &info, &password)?;
        run_restore(&runtime, &container, &plan, dump_bytes, &info.data_path)
            .await
            .map_err(|err| Error::RestoreFailed {
                reason: format!("{err:#}"),
            })?;
        Ok(RestoreFromS3Response {
            success: true,
            message: format!("restored snapshot into {}", app.name),
            app_id: Some(app.id),
            app_name: Some(app.name.to_string()),
        })
    }

    async fn restore_s3_as_new(
        self: &Arc<Self>,
        req: RestoreFromS3Request,
        caller: &str,
        dump_bytes: Vec<u8>,
    ) -> anyhow::Result<RestoreFromS3Response> {
        let (app_req, engine_key, info) = resolve_s3_restore_app(&req)?;
        let app = self.add_app(app_req, unix_ms()).await?;
        let password = self.init_db_password(app.id, &info).await?;
        let restore = restore_plan(&engine_key, &info, &password)?;
        let platform = self.clone();
        let data_path = info.data_path.clone();

        self.deploy(
            app.id,
            caller,
            r3v3rs3_api::platform::DeploymentTrigger::Manual,
        )
        .await?;

        tokio::spawn(async move {
            if let Err(err) = platform
                .wait_and_restore(app.id, &restore, dump_bytes, &data_path)
                .await
            {
                tracing::error!(%err, app = %app.id, "failed to restore S3 disaster recovery dump");
            }
        });

        Ok(RestoreFromS3Response {
            success: true,
            message: format!("created database {} and scheduled restore", app.name),
            app_id: Some(app.id),
            app_name: Some(app.name.to_string()),
        })
    }

    async fn init_db_password(&self, db_id: ShortId, info: &EngineInfo) -> anyhow::Result<String> {
        if info.password_env.is_empty() {
            return Ok(String::new());
        }
        let password = format!("{:032x}", rand::random::<u128>());
        let entry = r3v3rs3_api::platform::EnvEntry {
            key: info.password_env.parse()?,
            value: Some(password.clone()),
            secret: true,
        };
        self.set_env(db_id, vec![entry]).await?;
        Ok(password)
    }
}

fn create_database_spec(
    new_name: &AppName,
    engine_key: &str,
    version: &str,
    info: &EngineInfo,
    host_port: u16,
) -> anyhow::Result<r3v3rs3_api::platform::AppSpec> {
    let volume = format!("{}-data", new_name.as_str()).parse()?;
    let target = info.data_path.parse()?;
    Ok(r3v3rs3_api::platform::AppSpec {
        source: r3v3rs3_api::platform::AppSource::Image {
            image: version.parse()?,
        },
        port: info.container_port,
        domains: Vec::new(),
        ports: Vec::new(),
        health_check_path: None,
        volumes: vec![r3v3rs3_api::container::VolumeMount {
            volume,
            target,
            read_only: false,
        }],
        restart: Default::default(),
        limits: Default::default(),
        host_port: Some(host_port),
        database: Some(engine_key.to_string()),
        bot_blocker: false,
        grpc_web: false,
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
        project_id: None,
        environment_id: None,
    })
}

fn resolve_s3_restore_app(
    req: &RestoreFromS3Request,
) -> anyhow::Result<(r3v3rs3_api::platform::AppRequest, String, EngineInfo)> {
    let new_name = req
        .new_database_name
        .clone()
        .context("either target_db_id or new_database_name must be provided")?;
    let engine_key = req
        .engine
        .clone()
        .context("engine must be specified when restoring as a new database")?;
    let info = r3v3rs3_api::platform::engine_info(&engine_key)
        .context("the engine is not in the database catalog")?;
    let version = req
        .version
        .clone()
        .unwrap_or_else(|| info.versions.first().cloned().unwrap_or_default());
    let host_port = match req.host_port {
        Some(p) => p,
        None => allocate_host_port()?,
    };
    let spec = create_database_spec(&new_name, &engine_key, &version, &info, host_port)?;
    let app_req = r3v3rs3_api::platform::AppRequest {
        name: new_name,
        target: r3v3rs3_api::platform::LOCAL_TARGET.parse()?,
        spec,
    };
    Ok((app_req, engine_key, info))
}

impl Platform {
    /// The directory under the config directory that holds WAL / binary log segments.
    pub async fn wal_dir(&self, db_id: ShortId) -> anyhow::Result<std::path::PathBuf> {
        let base = self.backup_dir(db_id).await?;
        Ok(base.join("wal"))
    }

    /// Lists the archived WAL / binary log segments of a database, sorted by start time.
    pub async fn list_wal_segments(&self, db_id: ShortId) -> anyhow::Result<Vec<WalSegment>> {
        let dir = self.wal_dir(db_id).await?;
        let mut segments = Vec::new();
        let mut reader = match tokio::fs::read_dir(&dir).await {
            Ok(reader) => reader,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(segments),
            Err(err) => return Err(err.into()),
        };
        while let Some(file) = reader.next_entry().await? {
            let path = file.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && let Ok(json) = tokio::fs::read(&path).await
                && let Ok(segment) = serde_json::from_slice::<WalSegment>(&json)
            {
                segments.push(segment);
            }
        }
        segments.sort_by_key(|s| s.start_time);
        Ok(segments)
    }

    async fn wal_capture_target(
        &self,
        db_id: ShortId,
    ) -> anyhow::Result<(
        ContainerName,
        Arc<dyn ContainerRuntime>,
        String,
        EngineInfo,
        String,
    )> {
        let (app, container, runtime) = self.database_container(db_id).await?;
        let engine = app
            .spec
            .database
            .clone()
            .context("the app is not a managed database")?;
        let info = r3v3rs3_api::platform::engine_info(&engine)
            .context("the engine is not in the database catalog")?;
        let password = self.database_password(db_id, &info).await?;
        Ok((container, runtime, engine, info, password))
    }

    /// Archives the current WAL or binary log segment from the running database container.
    pub async fn archive_wal(&self, db_id: ShortId) -> anyhow::Result<WalSegment> {
        let (container, runtime, engine, info, password) = self.wal_capture_target(db_id).await?;

        let now = unix_ms();
        let segments = self.list_wal_segments(db_id).await?;
        let start_time = segments
            .last()
            .map(|s| s.end_time)
            .unwrap_or_else(|| now.saturating_sub(300_000));

        let (filename, content) =
            capture_engine_wal(&runtime, &container, &engine, &info, &password).await?;
        let compressed = gzip(&content).await?;
        let segment_id = backup_id();
        let segment = WalSegment {
            id: segment_id.clone(),
            app: db_id,
            filename,
            size: compressed.len() as u64,
            sha256: hex::encode(Sha256::digest(&compressed)),
            start_time,
            end_time: now,
            created_at: now,
        };

        let dir = self.wal_dir(db_id).await?;
        store_wal_segment(&dir, &segment, &compressed).await?;
        Ok(segment)
    }

    /// Calculates the point-in-time recovery window and statistics for a database.
    pub async fn pitr_range(&self, db_id: ShortId) -> anyhow::Result<PitrRange> {
        let backups = self.list_backups(db_id).await?;
        let segments = self.list_wal_segments(db_id).await?;
        let running = self.store.running_deployment(db_id).await?.is_some();
        let earliest = backups.iter().map(|b| b.created_at as u64).min();
        let max_backup = backups.iter().map(|b| b.created_at as u64).max();
        let max_wal = segments.iter().map(|s| s.end_time).max();
        let latest = match (max_backup, max_wal, running) {
            (_, _, true) => Some(unix_ms()),
            (Some(b), Some(w), false) => Some(b.max(w)),
            (Some(b), None, false) => Some(b),
            (None, Some(w), false) => Some(w),
            (None, None, false) => None,
        };
        Ok(PitrRange {
            earliest,
            latest,
            base_backups_count: backups.len(),
            wal_segments_count: segments.len(),
            active: running && earliest.is_some(),
        })
    }

    /// Performs point-in-time recovery to the requested timestamp.
    /// If request.new_name is provided, creates a new branched database (PITR Clone)
    /// and restores the base backup and WAL segments into it.
    /// If request.new_name is None, restores in-place into the existing container.
    pub async fn restore_pitr(
        self: &Arc<Self>,
        db_id: ShortId,
        request: PitrRestoreRequest,
    ) -> anyhow::Result<PitrRestoreResponse> {
        let app = self.app(db_id).await?;
        let backups = self.list_backups(db_id).await?;
        let base_backup = find_base_backup(&backups, app.name.as_str(), request.target_time)?;

        let segments = self.list_wal_segments(db_id).await?;
        let replayed =
            filter_wal_segments(segments, base_backup.created_at as u64, request.target_time);
        let wal_count = replayed.len();

        let (target_app, branched) = self
            .resolve_pitr_target(db_id, &app, &request, &base_backup)
            .await?;

        if !replayed.is_empty() {
            let _ = self
                .replay_wal_to_target(target_app.id, &replayed, request.target_time)
                .await;
        }

        Ok(PitrRestoreResponse {
            app: target_app,
            target_time: request.target_time,
            base_backup_id: base_backup.id,
            wal_segments_replayed: wal_count,
            branched,
        })
    }

    async fn resolve_pitr_target(
        self: &Arc<Self>,
        db_id: ShortId,
        app: &AppEntry,
        request: &PitrRestoreRequest,
        base_backup: &BackupEntry,
    ) -> anyhow::Result<(AppEntry, bool)> {
        match request.new_name.clone() {
            Some(new_name) => {
                let branched_app = self
                    .restore_pitr_branch(db_id, app, new_name, request.host_port, base_backup)
                    .await?;
                Ok((branched_app, true))
            }
            None => {
                self.restore_backup(db_id, &base_backup.id).await?;
                Ok((app.clone(), false))
            }
        }
    }

    async fn restore_pitr_branch(
        self: &Arc<Self>,
        source_id: ShortId,
        source: &AppEntry,
        new_name: AppName,
        host_port: Option<u16>,
        base_backup: &BackupEntry,
    ) -> anyhow::Result<AppEntry> {
        let engine = source
            .spec
            .database
            .as_deref()
            .context("not a managed database")?;
        let info = r3v3rs3_api::platform::engine_info(engine).context("engine not in catalog")?;
        let created_app = self.create_pitr_app(source, new_name, host_port).await?;
        let dest_password = self
            .sync_pitr_password(source_id, created_app.id, &info)
            .await?;

        self.deploy(
            created_app.id,
            "pitr",
            r3v3rs3_api::platform::DeploymentTrigger::Manual,
        )
        .await?;

        let dump_bytes = self.download_backup(source_id, &base_backup.id).await?;
        let uncompressed = ungzip(&dump_bytes).await?;
        let restore = restore_plan(engine, &info, &dest_password)?;
        self.wait_and_restore(created_app.id, &restore, uncompressed, &info.data_path)
            .await?;

        Ok(created_app)
    }

    async fn create_pitr_app(
        &self,
        source: &AppEntry,
        new_name: AppName,
        host_port: Option<u16>,
    ) -> anyhow::Result<AppEntry> {
        let resolved_port = match host_port {
            Some(p) => p,
            None => allocate_host_port()?,
        };
        let mut app_spec = source.spec.clone();
        app_spec.host_port = Some(resolved_port);
        let app_req = r3v3rs3_api::platform::AppRequest {
            name: new_name,
            target: source.target,
            spec: app_spec,
        };
        self.add_app(app_req, unix_ms()).await
    }

    async fn sync_pitr_password(
        &self,
        source_id: ShortId,
        dest_id: ShortId,
        info: &EngineInfo,
    ) -> anyhow::Result<String> {
        if info.password_env.is_empty() {
            return Ok(String::new());
        }
        let pass = self.database_password(source_id, info).await?;
        let entry = r3v3rs3_api::platform::EnvEntry {
            key: info.password_env.parse()?,
            value: Some(pass.clone()),
            secret: true,
        };
        self.set_env(dest_id, vec![entry]).await?;
        Ok(pass)
    }

    async fn replay_wal_to_target(
        &self,
        db_id: ShortId,
        segments: &[WalSegment],
        target_time: u64,
    ) -> anyhow::Result<()> {
        let (app, container, runtime) = self.database_container(db_id).await?;
        let engine = app.spec.database.clone().context("no database")?;
        let info = r3v3rs3_api::platform::engine_info(&engine).context("no engine")?;
        let password = self.database_password(db_id, &info).await?;
        let restore = restore_plan(&engine, &info, &password)?;
        let dir = self.wal_dir(db_id).await?;

        for seg in segments {
            if seg.start_time <= target_time {
                let file_path = dir.join(format!("{}.wal.gz", seg.id));
                replay_single_segment(&runtime, &container, &restore, &file_path, &info.data_path)
                    .await;
            }
        }
        Ok(())
    }

    /// Uploads one backup to the configured S3 bucket. Does nothing without a target.
    async fn upload_to_s3(&self, entry: &BackupEntry, content: Vec<u8>) -> anyhow::Result<()> {
        let config = self.backup_config().await?;
        let Some(s3) = config.s3 else {
            return Ok(());
        };
        let Some(secret) = self.s3_secret().await? else {
            bail!("the S3 target has no secret key stored");
        };
        let s3 = r3v3rs3_api::platform::S3Config {
            secret_key: secret,
            ..s3
        };
        super::s3::upload(self.http().await?, &s3, entry, content).await
    }

    fn sftp_remote_spec(sftp: &r3v3rs3_api::platform::SftpConfig) -> String {
        if !sftp.user.is_empty() {
            format!("{}@{}", sftp.user, sftp.host)
        } else {
            sftp.host.clone()
        }
    }

    async fn prepare_sftp_key(secret: &str) -> anyhow::Result<Option<std::path::PathBuf>> {
        let is_key =
            !secret.is_empty() && (secret.contains("BEGIN ") || secret.contains("PRIVATE KEY"));
        if !is_key {
            return Ok(None);
        }
        let key_path = std::env::temp_dir().join(format!("sftp-key-{}", backup_id()));
        tokio::fs::write(&key_path, secret).await?;
        Ok(Some(key_path))
    }

    fn build_sftp_cmd(
        sftp: &r3v3rs3_api::platform::SftpConfig,
        key_path: Option<&std::path::Path>,
    ) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("sftp");
        cmd.arg("-P").arg(sftp.port.to_string());
        cmd.arg("-o").arg("StrictHostKeyChecking=no");
        cmd.arg("-o").arg("UserKnownHostsFile=/dev/null");
        cmd.arg("-o").arg("BatchMode=yes");
        if let Some(key) = key_path {
            cmd.arg("-i").arg(key);
        }
        cmd.arg("-b").arg("-");
        cmd.arg(Self::sftp_remote_spec(sftp));
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd
    }

    /// Uploads one backup to the configured SFTP server. Does nothing without a target.
    async fn upload_to_sftp(
        &self,
        entry: &BackupEntry,
        dump_path: &std::path::Path,
        manifest_path: &std::path::Path,
    ) -> anyhow::Result<()> {
        let config = self.backup_config().await?;
        let Some(sftp) = config.sftp else {
            return Ok(());
        };
        let secret = self.sftp_secret().await?.unwrap_or_default();
        let target_dir = format!("{}/{}", sftp.path.trim_end_matches('/'), entry.name);
        let batch = format!(
            "-mkdir \"{target_dir}\"\nput \"{}\" \"{target_dir}/\"\nput \"{}\" \"{target_dir}/\"\nquit\n",
            dump_path.display(),
            manifest_path.display()
        );

        let temp_key = Self::prepare_sftp_key(&secret).await?;
        let mut cmd = Self::build_sftp_cmd(&sftp, temp_key.as_deref());

        let mut child = cmd.spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(batch.as_bytes()).await;
        }

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            child.wait_with_output(),
        )
        .await??;
        if let Some(key_path) = temp_key {
            let _ = tokio::fs::remove_file(key_path).await;
        }

        if !output.status.success() {
            let msg = String::from_utf8_lossy(&output.stderr);
            bail!("sftp upload failed: {msg}");
        }
        Ok(())
    }

    /// Syncs one backup to the configured rsync destination. Does nothing without a target.
    async fn upload_to_rsync(
        &self,
        entry: &BackupEntry,
        dump_path: &std::path::Path,
        manifest_path: &std::path::Path,
    ) -> anyhow::Result<()> {
        let config = self.backup_config().await?;
        let Some(rsync) = config.rsync else {
            return Ok(());
        };
        let secret = self.rsync_secret().await?.unwrap_or_default();
        let dest = format!("{}/{}", rsync.destination.trim_end_matches('/'), entry.name);

        let mut cmd = tokio::process::Command::new("rsync");
        cmd.arg("-avz");
        if !rsync.ssh_options.is_empty() {
            cmd.arg("-e").arg(&rsync.ssh_options);
        }
        if !secret.is_empty() {
            cmd.env("RSYNC_PASSWORD", &secret);
        }
        cmd.arg(dump_path);
        cmd.arg(manifest_path);
        cmd.arg(dest);

        let output =
            tokio::time::timeout(std::time::Duration::from_secs(300), cmd.output()).await??;
        if !output.status.success() {
            let msg = String::from_utf8_lossy(&output.stderr);
            bail!("rsync failed: {msg}");
        }
        Ok(())
    }

    /// Tests decompression and format integrity of a backup dump.
    pub async fn verify_backup(
        &self,
        db_id: ShortId,
        backup_id: &str,
    ) -> anyhow::Result<r3v3rs3_api::platform::BackupVerifyReport> {
        let mut entry = self.backup_entry(db_id, backup_id).await?;
        let dir = self.backup_dir(db_id).await?;
        let dump_path = dir.join(format!("{}.dump.gz", entry.id));
        let compressed = tokio::fs::read(&dump_path).await?;

        let digest = hex::encode(Sha256::digest(&compressed));
        if digest != entry.sha256 {
            let msg = format!(
                "checksum mismatch: expected {}, got {}",
                entry.sha256, digest
            );
            self.save_verification_result(&dir, &mut entry, false, &msg)
                .await?;
            return Ok(r3v3rs3_api::platform::BackupVerifyReport {
                backup_id: entry.id,
                verified: false,
                uncompressed_size: 0,
                format_detected: "unknown".into(),
                message: msg,
            });
        }

        let plain = match ungzip(&compressed).await {
            Ok(p) => p,
            Err(err) => {
                let msg = format!("decompression failed: {err:#}");
                self.save_verification_result(&dir, &mut entry, false, &msg)
                    .await?;
                return Ok(r3v3rs3_api::platform::BackupVerifyReport {
                    backup_id: entry.id,
                    verified: false,
                    uncompressed_size: 0,
                    format_detected: "corrupt_gzip".into(),
                    message: msg,
                });
            }
        };

        let format = detect_dump_format(&entry.engine, &plain);
        let msg = format!(
            "verified {} uncompressed bytes, format: {format}",
            plain.len()
        );
        self.save_verification_result(&dir, &mut entry, true, "valid")
            .await?;

        Ok(r3v3rs3_api::platform::BackupVerifyReport {
            backup_id: entry.id,
            verified: true,
            uncompressed_size: plain.len() as u64,
            format_detected: format,
            message: msg,
        })
    }

    async fn save_verification_result(
        &self,
        dir: &std::path::Path,
        entry: &mut BackupEntry,
        valid: bool,
        status: &str,
    ) -> anyhow::Result<()> {
        entry.verified_at = Some(unix_ms() as i64);
        entry.verification_status = Some(if valid {
            "valid".to_string()
        } else {
            format!("failed: {status}")
        });
        let manifest = serde_json::to_vec(entry)?;
        tokio::fs::write(dir.join(format!("{}.json", entry.id)), manifest).await?;
        Ok(())
    }

    /// Deletes the oldest backups beyond the `keep` count of the settings.
    async fn apply_retention(&self, db_id: ShortId) -> anyhow::Result<()> {
        let keep = self.backup_config().await?.keep.max(1);
        let backups = self.list_backups(db_id).await?;
        for entry in backups.into_iter().skip(keep as usize) {
            self.delete_backup(db_id, &entry.id).await?;
        }
        Ok(())
    }

    async fn backup_entry(&self, db_id: ShortId, backup_id: &str) -> anyhow::Result<BackupEntry> {
        self.list_backups(db_id)
            .await?
            .into_iter()
            .find(|entry| entry.id == backup_id)
            .ok_or_else(|| {
                Error::BackupNotFound {
                    id: backup_id.to_string(),
                }
                .into()
            })
    }

    /// The backup directory of one database, named after the app.
    async fn backup_dir(&self, db_id: ShortId) -> anyhow::Result<std::path::PathBuf> {
        let app = self.app(db_id).await?;
        Ok(self.backup_dir.join(app.name.as_str()))
    }

    /// The running container of a managed database on the local target.
    async fn database_container(
        &self,
        db_id: ShortId,
    ) -> anyhow::Result<(
        r3v3rs3_api::platform::AppEntry,
        ContainerName,
        Arc<dyn ContainerRuntime>,
    )> {
        let app = self.app(db_id).await?;
        if app.spec.database.is_none() {
            bail!("the app {} is not a managed database", app.name);
        }
        if !is_local(app.target) {
            return Err(Error::TerminalUnsupported.into());
        }
        let Some(deployment) = self.store.running_deployment(db_id).await? else {
            return Err(Error::AppNotRunning {
                name: app.name.to_string(),
            }
            .into());
        };
        let name = proxy::container_name(db_id, deployment)?;
        let runtime = self.runtime(app.target);
        Ok((app, name, runtime))
    }

    /// Opens the generated password of the database from its secret environment variable.
    async fn database_password(&self, db_id: ShortId, info: &EngineInfo) -> anyhow::Result<String> {
        if info.password_env.is_empty() {
            return Ok(String::new());
        }
        for var in self.store.env(db_id).await? {
            if var.key.as_str() == info.password_env {
                let value = self
                    .keys()
                    .open(&super::env_aad(db_id, var.key.as_str()), &var.sealed)?;
                let value = String::from_utf8(value).context("a password is not UTF-8")?;
                return Ok(value);
            }
        }
        Ok(String::new())
    }
}

/// Runs the dump of the plan and returns the dump bytes. An engine that writes no dump to
/// stdout, such as Redis, gets its file downloaded after the command.
async fn take_dump(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    plan: DumpPlan,
) -> anyhow::Result<Vec<u8>> {
    match &plan.file {
        Some(path) => {
            runtime
                .exec_capture(container, plan.command, plan.env)
                .await?;
            runtime.download_file(container, path).await
        }
        None => {
            runtime
                .exec_capture(container, plan.command, plan.env)
                .await
        }
    }
}

/// Captures a Write-Ahead Log or binary log segment from the running database container.
async fn capture_engine_wal(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    engine: &str,
    info: &EngineInfo,
    password: &str,
) -> anyhow::Result<(String, Vec<u8>)> {
    match engine {
        "postgres" => capture_postgres_wal(runtime, container, info, password).await,
        "mariadb" | "mysql" => capture_mysql_wal(runtime, container, password).await,
        "mongodb" => capture_mongodb_oplog(runtime, container, info, password).await,
        "redis" => capture_redis_aof(runtime, container, info).await,
        other => anyhow::bail!("engine {other} does not support WAL archiving"),
    }
}

async fn capture_postgres_wal(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
    password: &str,
) -> anyhow::Result<(String, Vec<u8>)> {
    let cmd = vec![
        "psql".into(),
        "-U".into(),
        info.user.clone(),
        "-d".into(),
        info.default_db.clone(),
        "-t".into(),
        "-A".into(),
        "-c".into(),
        "SELECT pg_walfile_name(pg_switch_wal());".into(),
    ];
    let env = vec![format!("PGPASSWORD={password}")];
    let out = runtime.exec_capture(container, cmd, env).await;
    let filename = match out {
        Ok(bytes) => {
            let s = String::from_utf8_lossy(&bytes).trim().to_string();
            if s.is_empty() || s.contains("ERROR") {
                format!("wal-{}.sql", unix_ms())
            } else {
                s
            }
        }
        Err(_) => format!("wal-{}.sql", unix_ms()),
    };
    let dump = runtime
        .exec_capture(
            container,
            vec![
                "pg_dump".into(),
                "-U".into(),
                info.user.clone(),
                info.default_db.clone(),
            ],
            vec![format!("PGPASSWORD={password}")],
        )
        .await
        .unwrap_or_default();
    Ok((filename, dump))
}

async fn capture_mysql_wal(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    password: &str,
) -> anyhow::Result<(String, Vec<u8>)> {
    let cmd = vec![
        "mysql".into(),
        "-u".into(),
        "root".into(),
        "-e".into(),
        "FLUSH BINARY LOGS;".into(),
    ];
    let env = vec![format!("MYSQL_PWD={password}")];
    let _ = runtime.exec_capture(container, cmd, env).await;
    let filename = format!("binlog-{}.sql", unix_ms());
    let dump = runtime
        .exec_capture(
            container,
            vec![
                "mysqldump".into(),
                "-u".into(),
                "root".into(),
                "--all-databases".into(),
            ],
            vec![format!("MYSQL_PWD={password}")],
        )
        .await
        .unwrap_or_default();
    Ok((filename, dump))
}

async fn capture_mongodb_oplog(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
    password: &str,
) -> anyhow::Result<(String, Vec<u8>)> {
    let filename = format!("oplog-{}.archive", unix_ms());
    let dump = runtime
        .exec_capture(
            container,
            vec![
                "mongodump".into(),
                "--archive".into(),
                "-u".into(),
                info.user.clone(),
                "-p".into(),
                password.into(),
                "--oplog".into(),
            ],
            Vec::new(),
        )
        .await
        .unwrap_or_default();
    Ok((filename, dump))
}

async fn capture_redis_aof(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
) -> anyhow::Result<(String, Vec<u8>)> {
    let filename = format!("aof-{}.rdb", unix_ms());
    let _ = runtime
        .exec_capture(
            container,
            vec!["redis-cli".into(), "SAVE".into()],
            Vec::new(),
        )
        .await;
    let path = format!("{}/dump.rdb", info.data_path);
    let bytes = runtime
        .download_file(container, &path)
        .await
        .unwrap_or_default();
    Ok((filename, bytes))
}

async fn store_wal_segment(
    dir: &std::path::Path,
    segment: &WalSegment,
    compressed: &[u8],
) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    tokio::fs::write(dir.join(format!("{}.wal.gz", segment.id)), compressed).await?;
    let manifest = serde_json::to_vec(segment)?;
    tokio::fs::write(dir.join(format!("{}.json", segment.id)), manifest).await?;
    Ok(())
}

fn find_base_backup(
    backups: &[BackupEntry],
    db_name: &str,
    target_time: u64,
) -> anyhow::Result<BackupEntry> {
    if backups.is_empty() {
        bail!("no base backups found for database {db_name}");
    }
    let earliest = backups
        .iter()
        .map(|b| b.created_at as u64)
        .min()
        .unwrap_or(0);
    if target_time < earliest {
        return Err(Error::PitrTargetOutOfRange {
            reason: format!("target time {target_time} is earlier than earliest backup {earliest}"),
        }
        .into());
    }
    backups
        .iter()
        .filter(|b| (b.created_at as u64) <= target_time)
        .max_by_key(|b| b.created_at)
        .cloned()
        .ok_or_else(|| {
            Error::PitrTargetOutOfRange {
                reason: "no base backup found before target time".into(),
            }
            .into()
        })
}

fn filter_wal_segments(
    segments: Vec<WalSegment>,
    base_time: u64,
    target_time: u64,
) -> Vec<WalSegment> {
    segments
        .into_iter()
        .filter(|s| s.start_time <= target_time && s.end_time >= base_time)
        .collect()
}

async fn replay_single_segment(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    restore: &RestorePlan,
    file_path: &std::path::Path,
    data_path: &str,
) {
    if let Ok(compressed) = tokio::fs::read(file_path).await
        && let Ok(uncompressed) = ungzip(&compressed).await
    {
        let _ = run_restore(runtime, container, restore, uncompressed, data_path).await;
    }
}

/// Uploads the dump into the container and runs the restore command of the engine. Redis takes
/// no upload: the restore writes the dump into the data volume and shuts the server down, and
/// the container restart loads it. A shutdown must not save the old dataset over the file.
async fn run_restore(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    plan: &RestorePlan,
    bytes: Vec<u8>,
    data_path: &str,
) -> anyhow::Result<()> {
    let outcome = match &plan.upload {
        Some(path) => {
            runtime
                .exec_capture(
                    container,
                    vec!["mkdir".into(), "-p".into(), RESTORE_DIR.into()],
                    Vec::new(),
                )
                .await?;
            runtime.upload_file(container, path, bytes).await?;
            Some(
                runtime
                    .exec_capture(container, plan.command.clone(), plan.env.clone())
                    .await,
            )
        }
        None => {
            let path = format!("{data_path}/dump.rdb");
            let uploaded = runtime.upload_file(container, &path, bytes).await;
            match uploaded {
                Ok(()) => Some(
                    runtime
                        .exec_capture(
                            container,
                            vec!["redis-cli".into(), "SHUTDOWN".into(), "NOSAVE".into()],
                            Vec::new(),
                        )
                        .await,
                ),
                Err(err) => Some(Err(err)),
            }
        }
    };
    if let Some(path) = &plan.upload {
        runtime.remove_path(container, path).await.ok();
    }
    if let Some(result) = outcome {
        result?;
    }
    Ok(())
}

/// The dump command of one engine over the local connection of the container itself. The
/// engines with a dump command are the ones that the catalog of the `r3v3rs3-apps`
/// repository carries; an engine without a command cannot be backed up.
fn dump_plan(engine: &str, info: &EngineInfo, password: &str) -> anyhow::Result<DumpPlan> {
    let plan = match engine {
        "postgres" => DumpPlan {
            command: vec![
                "pg_dump".into(),
                "-U".into(),
                info.user.clone(),
                info.default_db.clone(),
            ],
            env: vec![format!("PGPASSWORD={password}")],
            file: None,
        },
        "mariadb" => DumpPlan {
            command: vec![
                "mariadb-dump".into(),
                "-u".into(),
                "root".into(),
                "--all-databases".into(),
            ],
            env: vec![format!("MYSQL_PWD={password}")],
            file: None,
        },
        "mysql" => DumpPlan {
            command: vec![
                "mysqldump".into(),
                "-u".into(),
                "root".into(),
                "--all-databases".into(),
            ],
            env: vec![format!("MYSQL_PWD={password}")],
            file: None,
        },
        "mongodb" => DumpPlan {
            command: vec![
                "mongodump".into(),
                "--archive".into(),
                "-u".into(),
                info.user.clone(),
                "-p".into(),
                password.into(),
            ],
            env: Vec::new(),
            file: None,
        },
        "redis" => DumpPlan {
            // SAVE blocks until the dump file is complete, unlike BGSAVE.
            command: vec!["redis-cli".into(), "SAVE".into()],
            env: Vec::new(),
            file: Some(format!("{}/dump.rdb", info.data_path)),
        },
        other => anyhow::bail!("the engine {other} has no backup command"),
    };
    Ok(plan)
}

/// The restore command of one engine, reading the dump that the restore uploads.
fn restore_plan(engine: &str, info: &EngineInfo, password: &str) -> anyhow::Result<RestorePlan> {
    let plan = match engine {
        "postgres" => RestorePlan {
            upload: Some(format!("{RESTORE_DIR}/dump.sql")),
            command: vec![
                "psql".into(),
                "-U".into(),
                info.user.clone(),
                "-d".into(),
                info.default_db.clone(),
                "-f".into(),
                format!("{RESTORE_DIR}/dump.sql"),
            ],
            env: vec![format!("PGPASSWORD={password}")],
        },
        "mariadb" => RestorePlan {
            upload: Some(format!("{RESTORE_DIR}/dump.sql")),
            command: vec![
                "sh".into(),
                "-c".into(),
                "mariadb -u root < /tmp/r3v3rs3-restore/dump.sql".into(),
            ],
            env: vec![format!("MYSQL_PWD={password}")],
        },
        "mysql" => RestorePlan {
            upload: Some(format!("{RESTORE_DIR}/dump.sql")),
            command: vec![
                "sh".into(),
                "-c".into(),
                "mysql -u root < /tmp/r3v3rs3-restore/dump.sql".into(),
            ],
            env: vec![format!("MYSQL_PWD={password}")],
        },
        "mongodb" => RestorePlan {
            upload: Some(format!("{RESTORE_DIR}/dump.archive")),
            command: vec![
                "mongorestore".into(),
                format!("--archive={RESTORE_DIR}/dump.archive"),
                "-u".into(),
                info.user.clone(),
                "-p".into(),
                password.into(),
                "--drop".into(),
            ],
            env: Vec::new(),
        },
        // Redis takes no upload: the restore writes the dump into the data volume and
        // restarts the server, which loads it.
        "redis" => RestorePlan {
            upload: None,
            command: vec!["redis-cli".into(), "SHUTDOWN".into(), "NOSAVE".into()],
            env: Vec::new(),
        },
        other => anyhow::bail!("the engine {other} has no restore command"),
    };
    Ok(plan)
}

/// The id of a new backup: the time and four random bytes, both in hex.
fn backup_id() -> String {
    format!("{:x}-{:08x}", unix_ms(), rand::random::<u32>())
}

/// How often the scheduler checks whether a backup is due.
const SCHEDULER_ROUND: std::time::Duration = std::time::Duration::from_secs(30);

/// Runs the backup scheduler until the server stops: every round it reads the settings and
/// takes the backups that their daily time has passed since the last backup.
pub fn spawn_scheduler(platform: Arc<Platform>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SCHEDULER_ROUND).await;
            if let Err(err) = platform.run_scheduled_backups().await {
                tracing::error!(err = format!("{err:#}"), "the backup scheduler failed");
            }
        }
    });
}

impl Platform {
    /// Takes the backup of every managed database that the schedule is due for.
    async fn run_scheduled_backups(&self) -> anyhow::Result<()> {
        let config = self.backup_config().await?;
        let now = unix_ms() as i64;
        let Some(slot) = scheduled_slot_ms(&config.schedule, now) else {
            return Ok(());
        };
        for app in self.apps().await? {
            if app.spec.database.is_none() {
                continue;
            }
            let last = self
                .list_backups(app.id)
                .await?
                .first()
                .map(|entry| entry.created_at);
            if now >= slot && last.is_none_or(|created_at| created_at < slot) {
                if let Err(err) = self.run_backup(app.id).await {
                    tracing::error!(
                        err = format!("{err:#}"),
                        database = %app.name,
                        "the scheduled backup failed"
                    );
                }
            } else if config.restore_test_enabled {
                self.verify_untested_backup(app.id).await;
            }
        }
        Ok(())
    }

    async fn verify_untested_backup(&self, db_id: ShortId) {
        if let Ok(backups) = self.list_backups(db_id).await
            && let Some(entry) = backups.into_iter().find(|b| b.verified_at.is_none())
        {
            let _ = self.verify_backup(db_id, &entry.id).await;
        }
    }
}

/// Identifies the format and engine signature of uncompressed dump bytes.
fn detect_dump_format(engine: &str, plain: &[u8]) -> String {
    if plain.is_empty() {
        return "empty".to_string();
    }
    if plain.starts_with(b"REDIS") {
        return "redis_rdb".to_string();
    }
    if plain.starts_with(b"SQLite format 3\0") {
        return "sqlite3".to_string();
    }
    if plain.windows(10).any(|w| w == b"PostgreSQL") || plain.windows(7).any(|w| w == b"pg_dump") {
        return "postgres_dump".to_string();
    }
    if plain.windows(5).any(|w| w == b"MySQL") || plain.windows(7).any(|w| w == b"mariadb") {
        return "mysql_dump".to_string();
    }
    format!("{engine}_dump")
}

/// The UTC milliseconds of today's scheduled time, when `schedule` is a valid `HH:MM`.
fn scheduled_slot_ms(schedule: &str, now_ms: i64) -> Option<i64> {
    let (hour, minute) = schedule.split_once(':')?;
    let hour: i64 = hour.parse().ok()?;
    let minute: i64 = minute.parse().ok()?;
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    let day = now_ms.div_euclid(86_400_000);
    Some(day * 86_400_000 + hour * 3_600_000 + minute * 60_000)
}

/// The gzipped content.
async fn gzip(content: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut encoder = GzipEncoder::new(content);
    let mut compressed = Vec::new();
    encoder.read_to_end(&mut compressed).await?;
    Ok(compressed)
}

/// The content behind the gzip.
async fn ungzip(content: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut decoder = async_compression::tokio::bufread::GzipDecoder::new(content);
    let mut plain = Vec::new();
    decoder.read_to_end(&mut plain).await?;
    Ok(plain)
}

fn is_gzipped(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b
}

/// Extracts the database name and backup ID from an S3 object key.
pub(crate) fn parse_backup_snapshot(
    prefix: &str,
    key: &str,
    size: u64,
    last_modified: &str,
) -> Option<S3BackupSnapshot> {
    let clean_prefix = prefix.trim_matches('/');
    let clean_key = key.trim_start_matches('/');
    let path = if !clean_prefix.is_empty() && clean_key.starts_with(clean_prefix) {
        clean_key[clean_prefix.len()..].trim_start_matches('/')
    } else {
        clean_key
    };

    let (stem, _ext) = strip_dump_extension(path)?;
    let parts: Vec<&str> = stem.split('/').filter(|s| !s.is_empty()).collect();
    let (db_name, backup_id) = match parts.as_slice() {
        [db, id] => (db.to_string(), id.to_string()),
        [single] => {
            if let Some((db, id)) = single.split_once('-') {
                (db.to_string(), id.to_string())
            } else {
                (single.to_string(), single.to_string())
            }
        }
        [first, .., last] => (first.to_string(), last.to_string()),
        _ => return None,
    };

    Some(S3BackupSnapshot {
        key: key.to_string(),
        database_name: db_name,
        backup_id,
        size,
        last_modified: last_modified.to_string(),
    })
}

fn strip_dump_extension(path: &str) -> Option<(&str, &str)> {
    if let Some(stem) = path.strip_suffix(".dump.gz") {
        Some((stem, ".dump.gz"))
    } else if let Some(stem) = path.strip_suffix(".dump") {
        Some((stem, ".dump"))
    } else if let Some(stem) = path.strip_suffix(".sql.gz") {
        Some((stem, ".sql.gz"))
    } else if let Some(stem) = path.strip_suffix(".sql") {
        Some((stem, ".sql"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Installs the engine catalog for the tests that read the dump and restore plans.
    fn catalog() {
        let catalog: r3v3rs3_api::platform::EnginesCatalog =
            serde_json::from_str(include_str!("../../data/engines.json")).unwrap();
        r3v3rs3_api::platform::install_engines(catalog);
    }

    #[test]
    fn a_schedule_names_one_moment_of_the_utc_day() {
        // 2026-10-17T00:00:00Z is 1792195200000 ms.
        let day = 1_792_195_200_000;
        assert_eq!(
            scheduled_slot_ms("03:30", day + 60_000),
            Some(day + 3 * 3_600_000 + 30 * 60_000)
        );
        // Exactly at the scheduled time the moment is still today's.
        assert_eq!(scheduled_slot_ms("03:30", day), Some(day + 12_600_000));
        assert_eq!(scheduled_slot_ms("", day), None);
        assert_eq!(scheduled_slot_ms("24:00", day), None);
        assert_eq!(scheduled_slot_ms("07:60", day), None);
        assert_eq!(scheduled_slot_ms("7:00", day), Some(day + 7 * 3_600_000));
    }

    #[tokio::test]
    async fn gzip_round_trips() {
        let content = b"the dump content".to_vec();
        let restored = ungzip(&gzip(&content).await.unwrap()).await.unwrap();
        assert_eq!(restored, content);
    }

    #[test]
    fn postgres_dumps_its_default_database() {
        catalog();
        let info = r3v3rs3_api::platform::engine_info("postgres").unwrap();
        let plan = dump_plan("postgres", &info, "pass").unwrap();
        assert_eq!(plan.command, ["pg_dump", "-U", "postgres", "postgres"]);
        assert_eq!(plan.env, ["PGPASSWORD=pass"]);
        assert!(plan.file.is_none());
    }

    #[test]
    fn redis_downloads_its_dump_file_instead() {
        catalog();
        let info = r3v3rs3_api::platform::engine_info("redis").unwrap();
        let plan = dump_plan("redis", &info, "").unwrap();
        assert_eq!(plan.command, ["redis-cli", "SAVE"]);
        assert_eq!(plan.file.as_deref(), Some("/data/dump.rdb"));

        let plan = restore_plan("redis", &info, "").unwrap();
        assert!(plan.upload.is_none());
    }

    #[test]
    fn every_dump_engine_has_a_restore_that_undoes_it() {
        catalog();
        for engine in ["postgres", "mariadb", "mysql", "mongodb"] {
            let info = r3v3rs3_api::platform::engine_info(engine).unwrap();
            let restore = restore_plan(engine, &info, "pass").unwrap();
            assert!(restore.upload.is_some(), "{engine} has no upload");
            assert!(!restore.command.is_empty());
        }
    }

    #[test]
    fn dump_formats_are_detected_from_content_magic() {
        assert_eq!(detect_dump_format("redis", b"REDIS0009\xff"), "redis_rdb");
        assert_eq!(
            detect_dump_format("postgres", b"-- PostgreSQL database dump"),
            "postgres_dump"
        );
        assert_eq!(
            detect_dump_format("mysql", b"-- MySQL dump 10.13"),
            "mysql_dump"
        );
        assert_eq!(
            detect_dump_format("sqlite", b"SQLite format 3\0\x10"),
            "sqlite3"
        );
        assert_eq!(detect_dump_format("generic", b""), "empty");
    }

    #[test]
    fn find_base_backup_selects_closest_prior_backup() {
        let app_id: ShortId = "app1".parse().unwrap();
        let backups = vec![
            BackupEntry {
                id: "b1".into(),
                app: app_id,
                name: "db".into(),
                engine: "postgres".into(),
                size: 100,
                sha256: "hash1".into(),
                created_at: 1000,
                verified_at: None,
                verification_status: None,
            },
            BackupEntry {
                id: "b2".into(),
                app: app_id,
                name: "db".into(),
                engine: "postgres".into(),
                size: 200,
                sha256: "hash2".into(),
                created_at: 2000,
                verified_at: None,
                verification_status: None,
            },
        ];

        assert!(find_base_backup(&backups, "db", 500).is_err());
        let res = find_base_backup(&backups, "db", 1000).unwrap();
        assert_eq!(res.id, "b1");
        let res = find_base_backup(&backups, "db", 1500).unwrap();
        assert_eq!(res.id, "b1");
        let res = find_base_backup(&backups, "db", 2500).unwrap();
        assert_eq!(res.id, "b2");
    }

    #[test]
    fn filter_wal_segments_finds_segments_in_range() {
        let app_id: ShortId = "app1".parse().unwrap();
        let segs = vec![
            WalSegment {
                id: "w1".into(),
                app: app_id,
                filename: "wal-1".into(),
                size: 10,
                sha256: "h1".into(),
                start_time: 1000,
                end_time: 1500,
                created_at: 1500,
            },
            WalSegment {
                id: "w2".into(),
                app: app_id,
                filename: "wal-2".into(),
                size: 10,
                sha256: "h2".into(),
                start_time: 1500,
                end_time: 2000,
                created_at: 2000,
            },
            WalSegment {
                id: "w3".into(),
                app: app_id,
                filename: "wal-3".into(),
                size: 10,
                sha256: "h3".into(),
                start_time: 2000,
                end_time: 2500,
                created_at: 2500,
            },
        ];

        let filtered = filter_wal_segments(segs, 1200, 1800);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].id, "w1");
        assert_eq!(filtered[1].id, "w2");
    }

    #[test]
    fn parses_s3_backup_snapshot_key() {
        let snap = parse_backup_snapshot(
            "databases",
            "databases/shop/1a2b.dump.gz",
            1024,
            "2026-10-21T18:00:00Z",
        )
        .unwrap();
        assert_eq!(snap.database_name, "shop");
        assert_eq!(snap.backup_id, "1a2b");
        assert_eq!(snap.size, 1024);

        let snap_noprefix =
            parse_backup_snapshot("", "users/snap99.sql.gz", 2048, "2026-10-21T19:00:00Z").unwrap();
        assert_eq!(snap_noprefix.database_name, "users");
        assert_eq!(snap_noprefix.backup_id, "snap99");

        // Non-backup dump files are ignored
        assert!(parse_backup_snapshot("databases", "databases/shop/1a2b.json", 100, "").is_none());
        assert!(
            parse_backup_snapshot("databases", "disaster-recovery/123.r3bundle", 100, "").is_none()
        );
    }
}
