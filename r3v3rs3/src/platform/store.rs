//! The SQLite database of the deployment platform: the targets, the apps, their environment
//! variables and their deployments. The schema is created at the first connection, and every query
//! binds its values.

use anyhow::{Context as _, anyhow};
use r3v3rs3_api::container::AppName;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppHook, AppSpec, DeploymentEntry, DeploymentStatus, DeploymentTrigger,
    ImageUpdateStatus, LOCAL_TARGET, TargetEntry, TargetKind,
};
use serde_derive::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use std::path::Path;

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS targets (
        id               TEXT PRIMARY KEY,
        name             TEXT NOT NULL UNIQUE,
        kind             TEXT NOT NULL,
        token_hash       TEXT,
        cert_fingerprint TEXT,
        last_seen_at     INTEGER,
        created_at       INTEGER NOT NULL,
        drained          INTEGER NOT NULL DEFAULT 0
    )",
    "CREATE TABLE IF NOT EXISTS apps (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL UNIQUE,
        target_id  TEXT NOT NULL REFERENCES targets (id),
        spec       TEXT NOT NULL,
        proxy_id   TEXT,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS app_env (
        app_id TEXT    NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
        key    TEXT    NOT NULL,
        value  BLOB    NOT NULL,
        secret INTEGER NOT NULL,
        PRIMARY KEY (app_id, key)
    )",
    "CREATE TABLE IF NOT EXISTS deployments (
        id           TEXT PRIMARY KEY,
        app_id       TEXT    NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
        status       TEXT    NOT NULL,
        trigger      TEXT    NOT NULL,
        commit_sha   TEXT,
        image_digest TEXT,
        spec         TEXT    NOT NULL,
        env          BLOB    NOT NULL,
        username     TEXT    NOT NULL,
        message      TEXT,
        started_at   INTEGER NOT NULL,
        finished_at  INTEGER
    )",
    "CREATE INDEX IF NOT EXISTS deployments_app ON deployments (app_id, started_at)",
    "CREATE TABLE IF NOT EXISTS app_secrets (
        app_id TEXT NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
        name   TEXT NOT NULL,
        value  BLOB NOT NULL,
        PRIMARY KEY (app_id, name)
    )",
    "CREATE TABLE IF NOT EXISTS git_connections (
        id            TEXT PRIMARY KEY,
        name          TEXT NOT NULL UNIQUE,
        provider      TEXT NOT NULL,
        url           TEXT NOT NULL,
        client_id     TEXT NOT NULL,
        client_secret BLOB NOT NULL,
        status        TEXT NOT NULL,
        account       TEXT,
        access_token  BLOB,
        refresh_token BLOB,
        expires_at    INTEGER,
        redirect_uri  TEXT,
        created_at    INTEGER NOT NULL,
        updated_at    INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS github_apps (
        connection_id   TEXT PRIMARY KEY REFERENCES git_connections (id) ON DELETE CASCADE,
        html_url        TEXT    NOT NULL,
        private_key     BLOB    NOT NULL,
        installation_id INTEGER
    )",
    "CREATE TABLE IF NOT EXISTS platform_settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS app_hooks (
        app_id        TEXT PRIMARY KEY REFERENCES apps (id) ON DELETE CASCADE,
        connection_id TEXT    NOT NULL,
        repository    TEXT    NOT NULL,
        remote_id     TEXT,
        error         TEXT,
        failure       TEXT,
        updated_at    INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS app_metrics (
        app_id  TEXT NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
        ts      INTEGER NOT NULL,
        running INTEGER NOT NULL,
        health  TEXT    NOT NULL,
        cpu     REAL    NOT NULL,
        memory  INTEGER NOT NULL,
        rx_bytes INTEGER NOT NULL,
        tx_bytes INTEGER NOT NULL,
        requests INTEGER NOT NULL,
        PRIMARY KEY (app_id, ts)
    )",
    "CREATE INDEX IF NOT EXISTS app_metrics_app ON app_metrics (app_id, ts)",
    "CREATE TABLE IF NOT EXISTS app_updates (
        app_id           TEXT PRIMARY KEY REFERENCES apps (id) ON DELETE CASCADE,
        update_available INTEGER NOT NULL,
        current_tag      TEXT,
        latest_tag       TEXT,
        current_digest   TEXT,
        latest_digest    TEXT,
        checked_at       INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS app_scheduled_commands (
        id          TEXT PRIMARY KEY,
        app_id      TEXT NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
        name        TEXT NOT NULL,
        schedule    TEXT NOT NULL,
        command     TEXT NOT NULL,
        enabled     INTEGER NOT NULL,
        last_run_at INTEGER,
        last_status TEXT,
        last_output TEXT,
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS app_scheduled_commands_app ON app_scheduled_commands (app_id)",
    "CREATE TABLE IF NOT EXISTS registries (
        id         TEXT PRIMARY KEY,
        server     TEXT NOT NULL UNIQUE,
        username   TEXT NOT NULL,
        secret     BLOB NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS prune_policy (
        id                      TEXT PRIMARY KEY,
        enabled                 INTEGER NOT NULL,
        interval                TEXT NOT NULL,
        disk_pressure_threshold INTEGER,
        prune_images            INTEGER NOT NULL,
        prune_volumes           INTEGER NOT NULL,
        prune_build_cache       INTEGER NOT NULL,
        prune_containers        INTEGER NOT NULL,
        prune_logs              INTEGER NOT NULL DEFAULT 1,
        log_vacuum_threshold_mb INTEGER,
        container_log_max_size  TEXT,
        container_log_max_file  TEXT,
        build_cache_keep_storage_mb INTEGER,
        kept_builds_count           INTEGER,
        dangling_layers_only        INTEGER NOT NULL DEFAULT 1,
        last_pruned_at          INTEGER,
        last_pressure_pruned_at INTEGER,
        last_reclaimed_bytes    INTEGER
    )",
    "CREATE TABLE IF NOT EXISTS env_groups (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL UNIQUE,
        description TEXT,
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS env_group_vars (
        group_id TEXT    NOT NULL REFERENCES env_groups (id) ON DELETE CASCADE,
        key      TEXT    NOT NULL,
        value    BLOB    NOT NULL,
        secret   INTEGER NOT NULL,
        PRIMARY KEY (group_id, key)
    )",
    "CREATE TABLE IF NOT EXISTS projects (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL UNIQUE,
        description TEXT,
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS project_environments (
        id          TEXT PRIMARY KEY,
        project_id  TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
        name        TEXT NOT NULL,
        description TEXT,
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL,
        UNIQUE (project_id, name)
    )",
    "CREATE TABLE IF NOT EXISTS project_env_vars (
        environment_id TEXT NOT NULL REFERENCES project_environments (id) ON DELETE CASCADE,
        key            TEXT NOT NULL,
        value          BLOB NOT NULL,
        secret         INTEGER NOT NULL,
        PRIMARY KEY (environment_id, key)
    )",
    "CREATE TABLE IF NOT EXISTS outbound_webhooks (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        url        TEXT NOT NULL,
        secret     BLOB,
        enabled    INTEGER NOT NULL,
        events     TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS node_alert_rules (
        id                            TEXT PRIMARY KEY,
        enabled                       INTEGER NOT NULL,
        min_free_disk_percent         INTEGER,
        bandwidth_spike_bytes_per_sec INTEGER,
        max_fd_ratio_percent          INTEGER,
        max_time_wait_sockets         INTEGER
    )",
    "CREATE TABLE IF NOT EXISTS mcp_clients (
        id            TEXT PRIMARY KEY,
        name          TEXT NOT NULL,
        redirect_uris TEXT NOT NULL,
        created_at    INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS mcp_tokens (
        id                 TEXT PRIMARY KEY,
        access_token       TEXT NOT NULL UNIQUE,
        refresh_token      TEXT NOT NULL UNIQUE,
        client_id          TEXT NOT NULL,
        username           TEXT NOT NULL,
        scope              TEXT NOT NULL,
        expires_at         INTEGER NOT NULL,
        refresh_expires_at INTEGER NOT NULL,
        created_at         INTEGER NOT NULL
    )",
];

mod commands;
mod connections;
mod env_groups;
mod hooks;
mod metrics;
mod node_alerts;
mod projects;
mod prune;
mod registries;
mod webhooks;
pub use connections::{PUBLIC_URL, StoredConnection, StoredGithubApp, StoredTokens};
pub use env_groups::StoredEnvGroup;
pub use hooks::StoredHook;
pub use metrics::AppMetricRow;
pub use projects::{StoredProject, StoredProjectEnvironment};
pub use webhooks::StoredWebhook;

/// The secret of an app that holds its Git token.
pub const GIT_TOKEN: &str = "git_token";

/// The secret of an app that verifies the signatures of its webhook requests.
pub const WEBHOOK_SECRET: &str = "webhook_secret";

/// The query of the app entries, followed by `$rest`. A literal, because sqlx takes only a
/// static query.
macro_rules! select_apps {
    ($rest:literal) => {
        concat!(
            "SELECT id, name, target_id, spec, created_at, apps.updated_at AS updated_at,
                EXISTS (SELECT 1 FROM app_secrets s WHERE s.app_id = apps.id
                    AND s.name = 'git_token') AS git_token_set,
                EXISTS (SELECT 1 FROM app_secrets s WHERE s.app_id = apps.id
                    AND s.name = 'webhook_secret') AS webhook_secret_set,
                h.connection_id AS hook_connection, h.repository AS hook_repository,
                h.remote_id AS hook_remote_id, h.error AS hook_error, h.failure AS hook_failure,
                h.updated_at AS hook_updated_at,
                u.update_available AS update_available, u.current_tag AS update_current_tag,
                u.latest_tag AS update_latest_tag, u.current_digest AS update_current_digest,
                u.latest_digest AS update_latest_digest, u.checked_at AS update_checked_at
            FROM apps LEFT JOIN app_hooks h ON h.app_id = apps.id
            LEFT JOIN app_updates u ON u.app_id = apps.id ",
            $rest
        )
    };
}

/// One stored environment variable. The value is sealed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEnv {
    pub key: String,
    pub sealed: Vec<u8>,
    pub secret: bool,
}

/// A deployment at its start. The spec and the sealed environment are the snapshot that the
/// deployment and a later rollback run.
pub struct NewDeployment<'a> {
    pub id: ShortId,
    pub app: ShortId,
    pub status: DeploymentStatus,
    pub trigger: DeploymentTrigger,
    pub username: &'a str,
    pub spec: &'a AppSpec,
    pub env: &'a [StoredEnv],
    pub image_digest: Option<&'a str>,
    pub commit_sha: Option<&'a str>,
    pub message: Option<&'a str>,
    pub started_at: u64,
}

impl<'a> NewDeployment<'a> {
    pub fn new(
        app: &AppEntry,
        trigger: DeploymentTrigger,
        username: &'a str,
        spec: &'a AppSpec,
        env: &'a [StoredEnv],
    ) -> anyhow::Result<Self> {
        Ok(Self {
            id: super::new_id()?,
            app: app.id,
            status: DeploymentStatus::Queued,
            trigger,
            username,
            spec,
            env,
            image_digest: None,
            commit_sha: None,
            message: None,
            started_at: crate::clock::unix_ms(),
        })
    }

    pub fn with_source_info(mut self, digest: Option<&'a str>, commit: Option<&'a str>) -> Self {
        self.image_digest = digest;
        self.commit_sha = commit;
        self
    }

    pub fn pending(mut self, reason: &'a str) -> Self {
        self.status = DeploymentStatus::PendingApproval;
        self.message = Some(reason);
        self
    }
}

/// The failure message of a deployment that a stopped server left unfinished.
pub const UNFINISHED_MESSAGE: &str = "the server stopped during the deployment";

/// A deployment that a restart of the server failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfinishedDeployment {
    pub id: ShortId,
    pub app: ShortId,
    pub app_name: AppName,
    pub trigger: DeploymentTrigger,
}

/// A deployment whose container serves its app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningDeployment {
    pub id: ShortId,
    pub app: ShortId,
    /// The target of the app, which runs the container.
    pub target: ShortId,
    pub app_name: AppName,
    pub spec: AppSpec,
}

/// The outcome of a write that a unique app name can refuse.
#[derive(Debug, PartialEq, Eq)]
pub enum Write {
    Done,
    NotFound,
    NameTaken,
}

pub struct PlatformStore {
    pool: SqlitePool,
}

impl PlatformStore {
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePool::connect_with(options)
            .await
            .with_context(|| format!("failed to open {}", path.display()))?;
        for &statement in SCHEMA {
            sqlx::query(statement).execute(&pool).await?;
        }
        let _ = sqlx::query("ALTER TABLE targets ADD COLUMN drained INTEGER NOT NULL DEFAULT 0")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE targets ADD COLUMN region TEXT")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE prune_policy ADD COLUMN disk_pressure_threshold INTEGER")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE prune_policy ADD COLUMN last_pressure_pruned_at INTEGER")
            .execute(&pool)
            .await;
        let _ = sqlx::query(
            "ALTER TABLE prune_policy ADD COLUMN prune_logs INTEGER NOT NULL DEFAULT 1",
        )
        .execute(&pool)
        .await;
        let _ = sqlx::query("ALTER TABLE prune_policy ADD COLUMN log_vacuum_threshold_mb INTEGER")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE prune_policy ADD COLUMN container_log_max_size TEXT")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE prune_policy ADD COLUMN container_log_max_file TEXT")
            .execute(&pool)
            .await;
        let _ =
            sqlx::query("ALTER TABLE prune_policy ADD COLUMN build_cache_keep_storage_mb INTEGER")
                .execute(&pool)
                .await;
        let _ = sqlx::query("ALTER TABLE prune_policy ADD COLUMN kept_builds_count INTEGER")
            .execute(&pool)
            .await;
        let _ = sqlx::query(
            "ALTER TABLE prune_policy ADD COLUMN dangling_layers_only INTEGER NOT NULL DEFAULT 1",
        )
        .execute(&pool)
        .await;
        let _ = sqlx::query("ALTER TABLE node_alert_rules ADD COLUMN max_fd_ratio_percent INTEGER")
            .execute(&pool)
            .await;
        let _ =
            sqlx::query("ALTER TABLE node_alert_rules ADD COLUMN max_time_wait_sockets INTEGER")
                .execute(&pool)
                .await;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Adds the local target unless it exists.
    pub async fn ensure_local_target(&self, now: u64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO targets (id, name, kind, created_at, drained) VALUES (?, ?, 'local', ?, 0)
            ON CONFLICT (id) DO NOTHING",
        )
        .bind(LOCAL_TARGET)
        .bind(LOCAL_TARGET)
        .bind(sql_int(now))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn targets(&self) -> anyhow::Result<Vec<TargetEntry>> {
        let rows = sqlx::query(
            "SELECT id, name, kind, last_seen_at, cert_fingerprint IS NOT NULL AS enrolled, drained, region
            FROM targets ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(target_of).collect()
    }

    /// Sets or clears the geographical region of a target.
    pub async fn set_target_region(
        &self,
        id: ShortId,
        region: Option<&str>,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query("UPDATE targets SET region = ? WHERE id = ?")
            .bind(region)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Adds an agent target that waits for its enrollment with the token of `token_hash`.
    pub async fn add_agent_target(
        &self,
        id: ShortId,
        name: &str,
        token_hash: &str,
        now: u64,
    ) -> anyhow::Result<Write> {
        let result = sqlx::query(
            "INSERT INTO targets (id, name, kind, token_hash, created_at)
            VALUES (?, ?, 'agent', ?, ?)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(token_hash)
        .bind(sql_int(now))
        .execute(&self.pool)
        .await
        .map(|_| Write::Done);
        written_name(result, "targets.name")
    }

    /// Gives an agent target a new enrollment token. The certificate of the target stays valid
    /// until an agent enrolls with the new token.
    pub async fn set_target_token(&self, id: ShortId, token_hash: &str) -> anyhow::Result<bool> {
        let result =
            sqlx::query("UPDATE targets SET token_hash = ? WHERE id = ? AND kind = 'agent'")
                .bind(token_hash)
                .bind(id.to_string())
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() == 1)
    }

    /// The agent target whose enrollment token has `token_hash`.
    pub async fn target_by_token(&self, token_hash: &str) -> anyhow::Result<Option<ShortId>> {
        let row = sqlx::query("SELECT id FROM targets WHERE token_hash = ? AND kind = 'agent'")
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| id_of(&row, "id")).transpose()
    }

    /// Stores the certificate of an enrolled agent and clears the token, so the token enrolls
    /// only once. Returns false when another enrollment used the token first.
    pub async fn enroll_target(
        &self,
        id: ShortId,
        token_hash: &str,
        fingerprint: &str,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE targets SET cert_fingerprint = ?, token_hash = NULL
            WHERE id = ? AND token_hash = ?",
        )
        .bind(fingerprint)
        .bind(id.to_string())
        .bind(token_hash)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// The agent target of the client certificate with `fingerprint`.
    pub async fn target_by_fingerprint(
        &self,
        fingerprint: &str,
    ) -> anyhow::Result<Option<ShortId>> {
        let row =
            sqlx::query("SELECT id FROM targets WHERE cert_fingerprint = ? AND kind = 'agent'")
                .bind(fingerprint)
                .fetch_optional(&self.pool)
                .await?;
        row.map(|row| id_of(&row, "id")).transpose()
    }

    pub async fn target_seen(&self, id: ShortId, now: u64) -> anyhow::Result<()> {
        sqlx::query("UPDATE targets SET last_seen_at = ? WHERE id = ?")
            .bind(sql_int(now))
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Deletes an agent target. Returns false when no agent target has the id.
    pub async fn delete_target(&self, id: ShortId) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM targets WHERE id = ? AND kind = 'agent'")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Sets the drained state of a target for maintenance.
    pub async fn set_target_drained(&self, id: ShortId, drained: bool) -> anyhow::Result<bool> {
        let result = sqlx::query("UPDATE targets SET drained = ? WHERE id = ?")
            .bind(if drained { 1 } else { 0 })
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Whether an app runs on the target.
    pub async fn target_has_apps(&self, id: ShortId) -> anyhow::Result<bool> {
        let row = sqlx::query("SELECT 1 FROM apps WHERE target_id = ? LIMIT 1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    pub async fn has_target(&self, id: ShortId) -> anyhow::Result<bool> {
        let row = sqlx::query("SELECT 1 FROM targets WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    pub async fn apps(&self) -> anyhow::Result<Vec<AppEntry>> {
        let rows = sqlx::query(select_apps!("ORDER BY name"))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(app_of).collect()
    }

    pub async fn app(&self, id: ShortId) -> anyhow::Result<Option<AppEntry>> {
        let row = sqlx::query(select_apps!("WHERE id = ?"))
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(app_of).transpose()
    }

    /// The sealed value of a secret of an app.
    pub async fn secret(&self, app: ShortId, name: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let row = sqlx::query("SELECT value FROM app_secrets WHERE app_id = ? AND name = ?")
            .bind(app.to_string())
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|row| row.try_get("value")).transpose()?)
    }

    pub async fn set_secret(&self, app: ShortId, name: &str, sealed: &[u8]) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO app_secrets (app_id, name, value) VALUES (?, ?, ?)
            ON CONFLICT (app_id, name) DO UPDATE SET value = excluded.value",
        )
        .bind(app.to_string())
        .bind(name)
        .bind(sealed)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_secret(&self, app: ShortId, name: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM app_secrets WHERE app_id = ? AND name = ?")
            .bind(app.to_string())
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn insert_app(&self, app: &AppEntry) -> anyhow::Result<Write> {
        let result = sqlx::query(
            "INSERT INTO apps (id, name, target_id, spec, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(app.id.to_string())
        .bind(app.name.as_str())
        .bind(app.target.to_string())
        .bind(serde_json::to_string(&app.spec)?)
        .bind(sql_int(app.created_at))
        .bind(sql_int(app.updated_at))
        .execute(&self.pool)
        .await;
        written(result.map(|_| Write::Done))
    }

    /// Replaces the name, the target and the spec of an app.
    pub async fn update_app(&self, app: &AppEntry) -> anyhow::Result<Write> {
        let result = sqlx::query(
            "UPDATE apps SET name = ?, target_id = ?, spec = ?, updated_at = ? WHERE id = ?",
        )
        .bind(app.name.as_str())
        .bind(app.target.to_string())
        .bind(serde_json::to_string(&app.spec)?)
        .bind(sql_int(app.updated_at))
        .bind(app.id.to_string())
        .execute(&self.pool)
        .await;
        written(result.map(|done| match done.rows_affected() {
            0 => Write::NotFound,
            _ => Write::Done,
        }))
    }

    /// Deletes an app with its environment and its deployments. Returns false when the app does
    /// not exist.
    pub async fn delete_app(&self, id: ShortId) -> anyhow::Result<bool> {
        let done = sqlx::query("DELETE FROM apps WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn env(&self, app: ShortId) -> anyhow::Result<Vec<StoredEnv>> {
        let rows =
            sqlx::query("SELECT key, value, secret FROM app_env WHERE app_id = ? ORDER BY key")
                .bind(app.to_string())
                .fetch_all(&self.pool)
                .await?;
        rows.iter()
            .map(|row| {
                Ok(StoredEnv {
                    key: row.try_get("key")?,
                    sealed: row.try_get("value")?,
                    secret: row.try_get("secret")?,
                })
            })
            .collect()
    }

    /// Replaces every environment variable of an app in one transaction.
    pub async fn replace_env(&self, app: ShortId, env: &[StoredEnv]) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM app_env WHERE app_id = ?")
            .bind(app.to_string())
            .execute(&mut *tx)
            .await?;
        for var in env {
            sqlx::query("INSERT INTO app_env (app_id, key, value, secret) VALUES (?, ?, ?, ?)")
                .bind(app.to_string())
                .bind(&var.key)
                .bind(&var.sealed)
                .bind(var.secret)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The latest deployments of an app, the newest first.
    pub async fn deployments(
        &self,
        app: ShortId,
        limit: u32,
    ) -> anyhow::Result<Vec<DeploymentEntry>> {
        let rows = sqlx::query(
            "SELECT id, app_id, status, trigger, commit_sha, image_digest, username, message,
                started_at, finished_at
            FROM deployments WHERE app_id = ? ORDER BY started_at DESC, id LIMIT ?",
        )
        .bind(app.to_string())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(deployment_of).collect()
    }

    /// The latest deployments across all apps, the newest first.
    pub async fn recent_deployments(&self, limit: u32) -> anyhow::Result<Vec<DeploymentEntry>> {
        let rows = sqlx::query(
            "SELECT id, app_id, status, trigger, commit_sha, image_digest, username, message,
                started_at, finished_at
            FROM deployments ORDER BY started_at DESC, id LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(deployment_of).collect()
    }
}

impl PlatformStore {
    pub async fn insert_deployment(&self, deployment: &NewDeployment<'_>) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO deployments (id, app_id, status, trigger, image_digest, commit_sha, spec, env,
                username, message, started_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(deployment.id.to_string())
        .bind(deployment.app.to_string())
        .bind(deployment.status.as_str())
        .bind(deployment.trigger.as_str())
        .bind(deployment.image_digest)
        .bind(deployment.commit_sha)
        .bind(serde_json::to_string(deployment.spec)?)
        .bind(serde_json::to_vec(deployment.env)?)
        .bind(deployment.username)
        .bind(deployment.message)
        .bind(sql_int(deployment.started_at))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Moves a pending deployment to queued and clears its message.
    pub async fn approve_deployment(&self, id: ShortId, now: u64) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE deployments SET status = 'queued', message = NULL, started_at = ? WHERE id = ?",
        )
        .bind(sql_int(now))
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The deployments waiting for manual approval or for their deploy window.
    pub async fn pending_deployments(&self) -> anyhow::Result<Vec<DeploymentEntry>> {
        let rows = sqlx::query(
            "SELECT id, app_id, status, trigger, commit_sha, image_digest, username, message,
                started_at, finished_at
            FROM deployments WHERE status = 'pending_approval' ORDER BY started_at, id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(deployment_of).collect()
    }

    /// Sets the status of a deployment that has not finished.
    pub async fn set_status(&self, id: ShortId, status: DeploymentStatus) -> anyhow::Result<()> {
        sqlx::query("UPDATE deployments SET status = ? WHERE id = ?")
            .bind(status.as_str())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_image_digest(&self, id: ShortId, digest: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE deployments SET image_digest = ? WHERE id = ?")
            .bind(digest)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_commit_sha(&self, id: ShortId, sha: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE deployments SET commit_sha = ? WHERE id = ?")
            .bind(sha)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Ends a deployment with `failed` or `cancelled` and the reason.
    pub async fn fail_deployment(
        &self,
        id: ShortId,
        status: DeploymentStatus,
        message: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE deployments SET status = ?, message = ?, finished_at = ? WHERE id = ?")
            .bind(status.as_str())
            .bind(message)
            .bind(sql_int(now))
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Marks a deployment `running` and the running deployment of the same app `superseded`, in
    /// one transaction.
    pub async fn finish_deployment(
        &self,
        app: ShortId,
        id: ShortId,
        now: u64,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE deployments SET status = 'superseded'
            WHERE app_id = ? AND status = 'running' AND id <> ?",
        )
        .bind(app.to_string())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE deployments SET status = 'running', finished_at = ? WHERE id = ?")
            .bind(sql_int(now))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Ends the deployments that a stopped server left unfinished, and returns them.
    pub async fn fail_unfinished(&self, now: u64) -> anyhow::Result<Vec<UnfinishedDeployment>> {
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            "SELECT d.id, d.app_id, d.trigger, a.name FROM deployments d
                JOIN apps a ON a.id = d.app_id
            WHERE d.status IN ('queued', 'building', 'deploying')",
        )
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE deployments SET status = 'failed', finished_at = ?,
                message = ?
            WHERE status IN ('queued', 'building', 'deploying')",
        )
        .bind(sql_int(now))
        .bind(UNFINISHED_MESSAGE)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        rows.iter().map(unfinished_of).collect()
    }

    pub async fn deployment(&self, id: ShortId) -> anyhow::Result<Option<DeploymentEntry>> {
        let row = sqlx::query(
            "SELECT id, app_id, status, trigger, commit_sha, image_digest, username, message,
                started_at, finished_at
            FROM deployments WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(deployment_of).transpose()
    }

    /// The spec and the sealed environment that a deployment ran.
    pub async fn deployment_snapshot(
        &self,
        id: ShortId,
    ) -> anyhow::Result<Option<(AppSpec, Vec<StoredEnv>)>> {
        let row = sqlx::query("SELECT spec, env FROM deployments WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            let spec = serde_json::from_str(row.try_get("spec")?)?;
            let env = serde_json::from_slice(row.try_get("env")?)?;
            Ok((spec, env))
        })
        .transpose()
    }

    /// The id of the running deployment of an app.
    pub async fn running_deployment(&self, app: ShortId) -> anyhow::Result<Option<ShortId>> {
        let row = sqlx::query(
            "SELECT id FROM deployments WHERE app_id = ? AND status = 'running'
            ORDER BY started_at DESC LIMIT 1",
        )
        .bind(app.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| id_of(&row, "id")).transpose()
    }

    /// The running deployment of every app, with the app name.
    pub async fn running_deployments(&self) -> anyhow::Result<Vec<RunningDeployment>> {
        let rows = sqlx::query(
            "SELECT d.id, d.app_id, a.target_id, a.name, d.spec FROM deployments d
            JOIN apps a ON a.id = d.app_id
            WHERE d.status = 'running' ORDER BY a.name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(RunningDeployment {
                    id: id_of(row, "id")?,
                    app: id_of(row, "app_id")?,
                    target: id_of(row, "target_id")?,
                    app_name: row.try_get::<&str, _>("name")?.parse()?,
                    spec: serde_json::from_str(row.try_get("spec")?)?,
                })
            })
            .collect()
    }

    pub async fn set_image_update(
        &self,
        app_id: ShortId,
        status: &ImageUpdateStatus,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO app_updates (app_id, update_available, current_tag, latest_tag, current_digest, latest_digest, checked_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(app_id) DO UPDATE SET
                update_available = excluded.update_available,
                current_tag = excluded.current_tag,
                latest_tag = excluded.latest_tag,
                current_digest = excluded.current_digest,
                latest_digest = excluded.latest_digest,
                checked_at = excluded.checked_at",
        )
        .bind(app_id.to_string())
        .bind(status.update_available)
        .bind(&status.current_tag)
        .bind(&status.latest_tag)
        .bind(&status.current_digest)
        .bind(&status.latest_digest)
        .bind(sql_int(status.checked_at))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn clear_image_update(&self, app_id: ShortId) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM app_updates WHERE app_id = ?")
            .bind(app_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// Maps a unique constraint failure on the app name to `NameTaken`.
fn written(result: Result<Write, sqlx::Error>) -> anyhow::Result<Write> {
    written_name(result, "apps.name")
}

/// Maps a unique constraint failure on the `column` of a name to `NameTaken`.
fn written_name(result: Result<Write, sqlx::Error>, column: &str) -> anyhow::Result<Write> {
    match result {
        Err(sqlx::Error::Database(err))
            if err.is_unique_violation() && err.message().contains(column) =>
        {
            Ok(Write::NameTaken)
        }
        other => Ok(other?),
    }
}

/// SQLite stores signed 64-bit integers. A larger value is the largest integer.
fn sql_int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn id_of(row: &SqliteRow, column: &str) -> anyhow::Result<ShortId> {
    Ok(row.try_get::<&str, _>(column)?.parse()?)
}

fn time_of(row: &SqliteRow, column: &str) -> anyhow::Result<u64> {
    Ok(u64::try_from(row.try_get::<i64, _>(column)?)?)
}

fn optional_time_of(row: &SqliteRow, column: &str) -> anyhow::Result<Option<u64>> {
    let value = row.try_get::<Option<i64>, _>(column)?;
    Ok(value.map(u64::try_from).transpose()?)
}

fn target_of(row: &SqliteRow) -> anyhow::Result<TargetEntry> {
    let kind = match row.try_get::<&str, _>("kind")? {
        "local" => TargetKind::Local,
        "agent" => TargetKind::Agent,
        other => return Err(anyhow!("unknown target kind {other}")),
    };
    // The local target runs on the server itself, so it is enrolled and online.
    let local = kind == TargetKind::Local;
    let drained = row
        .try_get::<i64, _>("drained")
        .map(|v| v != 0)
        .unwrap_or(false);
    let region = row.try_get::<Option<String>, _>("region").ok().flatten();
    Ok(TargetEntry {
        id: id_of(row, "id")?,
        name: row.try_get("name")?,
        kind,
        last_seen_at: optional_time_of(row, "last_seen_at")?,
        enrolled: local || row.try_get::<bool, _>("enrolled")?,
        online: local,
        drained,
        version: None,
        latency_ms: None,
        jitter_ms: None,
        packet_loss_percent: None,
        quality: None,
        latency_samples: Vec::new(),
        region,
    })
}

fn unfinished_of(row: &SqliteRow) -> anyhow::Result<UnfinishedDeployment> {
    Ok(UnfinishedDeployment {
        id: id_of(row, "id")?,
        app: id_of(row, "app_id")?,
        app_name: row.try_get::<&str, _>("name")?.parse()?,
        trigger: parse_trigger(row.try_get("trigger")?)?,
    })
}

fn app_identity(row: &SqliteRow) -> anyhow::Result<(ShortId, AppName, ShortId)> {
    Ok((
        id_of(row, "id")?,
        row.try_get::<&str, _>("name")?.parse::<AppName>()?,
        id_of(row, "target_id")?,
    ))
}

fn app_of(row: &SqliteRow) -> anyhow::Result<AppEntry> {
    let (id, name, target) = app_identity(row)?;
    let spec: AppSpec = serde_json::from_str(row.try_get("spec")?)?;
    let (git_token_set, webhook_secret_set, hook) = app_extras(row)?;
    let update = app_update_of(row)?;
    Ok(AppEntry {
        id,
        name,
        target,
        spec,
        git_token_set,
        webhook_secret_set,
        hook,
        update,
        catalog_update: None,
        created_at: time_of(row, "created_at")?,
        updated_at: time_of(row, "updated_at")?,
    })
}

fn app_update_of(row: &SqliteRow) -> anyhow::Result<Option<ImageUpdateStatus>> {
    let checked_at: Option<i64> = row.try_get("update_checked_at")?;
    let Some(checked_at) = checked_at else {
        return Ok(None);
    };
    let update_available: bool = row.try_get("update_available")?;
    let current_tag: Option<String> = row.try_get("update_current_tag")?;
    let latest_tag: Option<String> = row.try_get("update_latest_tag")?;
    let current_digest: Option<String> = row.try_get("update_current_digest")?;
    let latest_digest: Option<String> = row.try_get("update_latest_digest")?;
    let checked_at = u64::try_from(checked_at).unwrap_or(0);
    Ok(Some(ImageUpdateStatus {
        update_available,
        current_tag,
        latest_tag,
        current_digest,
        latest_digest,
        checked_at,
    }))
}

/// Whether the app has a Git token, whether it has a webhook secret, and its webhook at its Git
/// provider.
fn app_extras(row: &SqliteRow) -> anyhow::Result<(bool, bool, Option<AppHook>)> {
    Ok((
        row.try_get("git_token_set")?,
        row.try_get("webhook_secret_set")?,
        hooks::app_hook_of(row)?,
    ))
}

fn deployment_of(row: &SqliteRow) -> anyhow::Result<DeploymentEntry> {
    let (status, trigger) = deployment_state(row)?;
    let (started_at, finished_at) = deployment_times(row)?;
    Ok(DeploymentEntry {
        id: id_of(row, "id")?,
        app: id_of(row, "app_id")?,
        status,
        trigger,
        commit_sha: row.try_get("commit_sha")?,
        image_digest: row.try_get("image_digest")?,
        username: row.try_get("username")?,
        started_at,
        finished_at,
        message: row.try_get("message")?,
    })
}

fn deployment_state(row: &SqliteRow) -> anyhow::Result<(DeploymentStatus, DeploymentTrigger)> {
    let status = parse_status(row.try_get("status")?)?;
    let trigger = parse_trigger(row.try_get("trigger")?)?;
    Ok((status, trigger))
}

fn deployment_times(row: &SqliteRow) -> anyhow::Result<(u64, Option<u64>)> {
    Ok((
        time_of(row, "started_at")?,
        optional_time_of(row, "finished_at")?,
    ))
}

fn parse_status(value: &str) -> anyhow::Result<DeploymentStatus> {
    serde_json::from_value(serde_json::Value::from(value))
        .with_context(|| format!("unknown deployment status {value}"))
}

fn parse_trigger(value: &str) -> anyhow::Result<DeploymentTrigger> {
    serde_json::from_value(serde_json::Value::from(value))
        .with_context(|| format!("unknown deployment trigger {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::platform::AppSource;

    async fn store() -> anyhow::Result<(PlatformStore, tempfile_path::TempPath)> {
        let path = tempfile_path::TempPath::new("platform-store");
        let store = PlatformStore::open(&path.0).await?;
        store.ensure_local_target(1).await?;
        Ok((store, path))
    }

    /// A database path under the temporary directory that is removed after the test.
    mod tempfile_path {
        pub struct TempPath(pub std::path::PathBuf);

        impl TempPath {
            pub fn new(name: &str) -> Self {
                let unique = hex::encode(rand::random::<[u8; 8]>());
                Self(std::env::temp_dir().join(format!("r3v3rs3-{name}-{unique}.db")))
            }
        }

        impl Drop for TempPath {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
    }

    fn app(id: &str, name: &str) -> AppEntry {
        AppEntry {
            id: id.parse().unwrap(),
            name: name.parse().unwrap(),
            target: LOCAL_TARGET.parse().unwrap(),
            spec: AppSpec {
                source: AppSource::Image {
                    image: "nginx:1.27".parse().unwrap(),
                },
                port: 80,
                domains: vec!["shop.example.com".parse().unwrap()],
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
                lifecycle_hooks: None,
            },
            git_token_set: false,
            webhook_secret_set: false,
            hook: None,
            update: None,
            catalog_update: None,
            created_at: 10,
            updated_at: 10,
        }
    }

    #[tokio::test]
    async fn image_update_can_be_saved_read_and_cleared() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        store.insert_app(&shop).await?;

        let status = ImageUpdateStatus {
            update_available: true,
            current_tag: Some("1.27".into()),
            latest_tag: Some("1.28".into()),
            current_digest: Some("sha256:1111".into()),
            latest_digest: Some("sha256:2222".into()),
            checked_at: 500,
        };
        store.set_image_update(shop.id, &status).await?;
        let loaded = store.app(shop.id).await?.unwrap();
        assert_eq!(loaded.update, Some(status));

        store.clear_image_update(shop.id).await?;
        let cleared = store.app(shop.id).await?.unwrap();
        assert!(cleared.update.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn a_second_secret_value_replaces_the_first() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        store.insert_app(&shop).await?;
        store.set_secret(shop.id, GIT_TOKEN, &[1]).await?;
        store.set_secret(shop.id, GIT_TOKEN, &[2]).await?;
        assert_eq!(store.secret(shop.id, GIT_TOKEN).await?, Some(vec![2]));
        let entry = store.app(shop.id).await?.context("app")?;
        assert!(entry.git_token_set && !entry.webhook_secret_set);
        Ok(())
    }

    #[tokio::test]
    async fn each_secret_has_its_own_flag() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        store.insert_app(&shop).await?;
        store.set_secret(shop.id, WEBHOOK_SECRET, &[4]).await?;
        let entry = &store.apps().await?[0];
        assert!(entry.webhook_secret_set && !entry.git_token_set);
        Ok(())
    }

    async fn edge_store() -> anyhow::Result<(PlatformStore, tempfile_path::TempPath, ShortId)> {
        let (store, path) = store().await?;
        let edge: ShortId = "fzn-txd".parse()?;
        assert_eq!(
            store.add_agent_target(edge, "edge", "hash-1", 5).await?,
            Write::Done
        );
        Ok((store, path, edge))
    }

    #[tokio::test]
    async fn an_enrollment_token_enrolls_one_certificate_once() -> anyhow::Result<()> {
        let (store, _path, edge) = edge_store().await?;
        let other: ShortId = "bcd-fgh".parse()?;
        let taken = store.add_agent_target(other, "edge", "hash-2", 5).await?;
        assert_eq!(taken, Write::NameTaken);
        assert_eq!(store.target_by_token("hash-1").await?, Some(edge));
        assert!(store.enroll_target(edge, "hash-1", "cert-1").await?);
        assert!(!store.enroll_target(edge, "hash-1", "cert-2").await?);
        assert_eq!(store.target_by_token("hash-1").await?, None);
        assert_eq!(store.target_by_fingerprint("cert-1").await?, Some(edge));
        assert_eq!(store.target_by_fingerprint("cert-2").await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn a_new_token_keeps_the_certificate_until_the_new_enrollment() -> anyhow::Result<()> {
        let (store, _path, edge) = edge_store().await?;
        assert!(store.enroll_target(edge, "hash-1", "cert-1").await?);
        assert!(store.set_target_token(edge, "hash-3").await?);
        assert_eq!(store.target_by_fingerprint("cert-1").await?, Some(edge));
        assert!(store.enroll_target(edge, "hash-3", "cert-3").await?);
        assert_eq!(store.target_by_fingerprint("cert-1").await?, None);
        // The local target has no token.
        let local: ShortId = LOCAL_TARGET.parse()?;
        assert!(!store.set_target_token(local, "hash-4").await?);
        Ok(())
    }

    #[tokio::test]
    async fn the_contact_time_of_an_agent_is_stored() -> anyhow::Result<()> {
        let (store, _path, edge) = edge_store().await?;
        store.target_seen(edge, 42).await?;
        let targets = store.targets().await?;
        let entry = targets.iter().find(|t| t.id == edge).context("edge")?;
        assert_eq!(entry.last_seen_at, Some(42));
        assert_eq!(entry.kind, TargetKind::Agent);
        Ok(())
    }

    #[tokio::test]
    async fn a_deleted_secret_is_gone() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        store.insert_app(&shop).await?;
        store.set_secret(shop.id, GIT_TOKEN, &[1]).await?;
        store.delete_secret(shop.id, GIT_TOKEN).await?;
        assert_eq!(store.secret(shop.id, GIT_TOKEN).await?, None);
        assert!(!store.apps().await?[0].git_token_set);
        Ok(())
    }

    #[tokio::test]
    async fn deleting_an_app_removes_its_secrets() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        store.insert_app(&shop).await?;
        store.set_secret(shop.id, GIT_TOKEN, &[3]).await?;
        store.delete_app(shop.id).await?;
        assert_eq!(store.secret(shop.id, GIT_TOKEN).await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn the_local_target_exists_once() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        store.ensure_local_target(2).await?;
        let targets = store.targets().await?;
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, TargetKind::Local);
        assert!(store.has_target(LOCAL_TARGET.parse()?).await?);
        assert!(!store.has_target("other".parse()?).await?);
        Ok(())
    }

    #[tokio::test]
    async fn an_app_round_trips_and_its_name_is_unique() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        assert_eq!(store.insert_app(&shop).await?, Write::Done);
        assert_eq!(store.app(shop.id).await?, Some(shop.clone()));
        let copy = app("ghj-klm", "shop");
        assert_eq!(store.insert_app(&copy).await?, Write::NameTaken);

        store.insert_app(&app("ghj-klm", "blog")).await?;
        let names = store
            .apps()
            .await?
            .into_iter()
            .map(|app| app.name.to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, ["blog", "shop"]);
        Ok(())
    }

    #[tokio::test]
    async fn an_update_keeps_the_names_unique() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        store.insert_app(&app("abc-def", "shop")).await?;
        let mut blog = app("ghj-klm", "blog");
        store.insert_app(&blog).await?;

        blog.name = "shop".parse()?;
        assert_eq!(store.update_app(&blog).await?, Write::NameTaken);
        blog.name = "news".parse()?;
        blog.updated_at = 20;
        assert_eq!(store.update_app(&blog).await?, Write::Done);
        assert_eq!(store.app(blog.id).await?, Some(blog));
        let missing = app("zzz-zzz", "none");
        assert_eq!(store.update_app(&missing).await?, Write::NotFound);
        Ok(())
    }

    #[tokio::test]
    async fn an_app_needs_an_existing_target() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let mut orphan = app("abc-def", "shop");
        orphan.target = "missing".parse()?;
        assert!(store.insert_app(&orphan).await.is_err());
        Ok(())
    }

    fn stored(key: &str, sealed: &[u8], secret: bool) -> StoredEnv {
        StoredEnv {
            key: key.into(),
            sealed: sealed.to_vec(),
            secret,
        }
    }

    #[tokio::test]
    async fn an_env_update_replaces_every_variable() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        store.insert_app(&shop).await?;
        let env = vec![stored("A", &[1, 2], false), stored("B", &[3], true)];
        store.replace_env(shop.id, &env).await?;
        assert_eq!(store.env(shop.id).await?, env);
        store.replace_env(shop.id, &env[1..]).await?;
        assert_eq!(store.env(shop.id).await?, env[1..]);
        Ok(())
    }

    #[tokio::test]
    async fn deleting_an_app_removes_its_environment_and_deployments() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        store.insert_app(&shop).await?;
        store
            .replace_env(shop.id, &[stored("A", &[1], false)])
            .await?;
        insert_deployment(&store, "dpl-bcd", shop.id).await?;

        assert!(store.delete_app(shop.id).await?);
        assert!(!store.delete_app(shop.id).await?);
        assert!(store.env(shop.id).await?.is_empty());
        assert!(store.deployments(shop.id, 10).await?.is_empty());
        Ok(())
    }

    async fn insert_deployment(
        store: &PlatformStore,
        id: &str,
        app: ShortId,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO deployments (id, app_id, status, trigger, image_digest, spec, env,
                username, message, started_at, finished_at)
            VALUES (?, ?, 'failed', 'rollback', 'sha256:ab', '{}', x'00', 'alice', 'no health', 5, 9)",
        )
        .bind(id)
        .bind(app.to_string())
        .execute(&store.pool)
        .await?;
        Ok(())
    }

    fn new_deployment<'a>(id: &str, app: &'a AppEntry, env: &'a [StoredEnv]) -> NewDeployment<'a> {
        NewDeployment {
            id: id.parse().unwrap(),
            app: app.id,
            status: DeploymentStatus::Queued,
            trigger: DeploymentTrigger::Manual,
            username: "alice",
            spec: &app.spec,
            env,
            image_digest: None,
            commit_sha: None,
            message: None,
            started_at: 5,
        }
    }

    /// Deploys `shop` twice. The first deployment has the environment and an image digest.
    async fn deploy_twice(
        store: &PlatformStore,
        shop: &AppEntry,
        env: &[StoredEnv],
    ) -> anyhow::Result<(ShortId, ShortId)> {
        store.insert_app(shop).await?;
        let first = new_deployment("fff-bcd", shop, env);
        store.insert_deployment(&first).await?;
        store.set_image_digest(first.id, "nginx@sha256:ab").await?;
        store.finish_deployment(shop.id, first.id, 7).await?;
        let second = new_deployment("fff-cdf", shop, &[]);
        store.insert_deployment(&second).await?;
        store.finish_deployment(shop.id, second.id, 9).await?;
        Ok((first.id, second.id))
    }

    #[tokio::test]
    async fn one_deployment_of_an_app_runs_and_keeps_its_snapshot() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        let env = [stored("A", &[1, 2], true)];
        let (first, second) = deploy_twice(&store, &shop, &env).await?;

        let running = store.running_deployments().await?;
        assert_eq!(running.len(), 1);
        assert_eq!(
            (running[0].id, running[0].app_name.as_str()),
            (second, "shop")
        );
        let old = store.deployment(first).await?.context("first")?;
        assert_eq!(old.status, DeploymentStatus::Superseded);
        assert_eq!(old.image_digest.as_deref(), Some("nginx@sha256:ab"));
        let (spec, sealed) = store
            .deployment_snapshot(first)
            .await?
            .context("snapshot")?;
        assert_eq!((spec, sealed), (shop.spec.clone(), env.to_vec()));
        Ok(())
    }

    #[tokio::test]
    async fn a_restart_fails_the_unfinished_deployments() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        store.insert_app(&shop).await?;
        let deployment = new_deployment("fff-bcd", &shop, &[]);
        store.insert_deployment(&deployment).await?;
        store
            .set_status(deployment.id, DeploymentStatus::Deploying)
            .await?;
        store.fail_unfinished(8).await?;
        let entry = store.deployment(deployment.id).await?.context("entry")?;
        assert_eq!(entry.status, DeploymentStatus::Failed);
        assert_eq!(entry.finished_at, Some(8));
        assert!(store.running_deployments().await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_deployment_row_maps_to_its_entry() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        store.insert_app(&shop).await?;
        insert_deployment(&store, "dpl-bcd", shop.id).await?;
        let expected = DeploymentEntry {
            id: "dpl-bcd".parse()?,
            app: shop.id,
            status: DeploymentStatus::Failed,
            trigger: DeploymentTrigger::Rollback,
            commit_sha: None,
            image_digest: Some("sha256:ab".into()),
            username: "alice".into(),
            started_at: 5,
            finished_at: Some(9),
            message: Some("no health".into()),
        };
        assert_eq!(store.deployments(shop.id, 10).await?, [expected]);
        Ok(())
    }

    #[tokio::test]
    async fn node_alert_rules_round_trip() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let default_rules = store.get_node_alert_rules().await?;
        assert!(default_rules.enabled);
        assert_eq!(default_rules.min_free_disk_percent, Some(10));
        assert_eq!(default_rules.max_fd_ratio_percent, Some(80));
        assert_eq!(default_rules.max_time_wait_sockets, None);

        let custom = r3v3rs3_api::platform::NodeAlertRules {
            enabled: false,
            min_free_disk_percent: Some(25),
            bandwidth_spike_bytes_per_sec: Some(50_000_000),
            max_fd_ratio_percent: Some(85),
            max_time_wait_sockets: Some(15_000),
        };
        store.save_node_alert_rules(&custom).await?;
        let fetched = store.get_node_alert_rules().await?;
        assert_eq!(fetched, custom);
        Ok(())
    }
}
