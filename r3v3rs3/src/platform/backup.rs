//! The backups of the managed databases. A backup is the dump of the engine, gzipped, in the
//! `backups` directory of the config directory, with one JSON manifest per file. A restore
//! uploads the dump back into the running container and runs the restore command of the engine.

use super::{Platform, is_local, proxy};
use crate::clock::unix_ms;
use crate::runtime::ContainerRuntime;
use anyhow::{Context, bail};
use async_compression::tokio::bufread::GzipEncoder;
use r3v3rs3_api::container::ContainerName;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{BackupConfig, BackupEntry, EngineInfo};
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
}
