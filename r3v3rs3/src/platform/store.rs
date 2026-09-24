//! The SQLite database of the deployment platform: the targets, the apps, their environment
//! variables and their deployments. The schema is created at the first connection, and every query
//! binds its values.

use anyhow::{Context as _, anyhow};
use r3v3rs3_api::container::AppName;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppSpec, DeploymentEntry, DeploymentStatus, DeploymentTrigger, LOCAL_TARGET,
    TargetEntry, TargetKind,
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
        created_at       INTEGER NOT NULL
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
];

/// The secret of an app that holds its Git token.
pub const GIT_TOKEN: &str = "git_token";

/// The query of the app entries, followed by `$rest`. A literal, because sqlx takes only a
/// static query.
macro_rules! select_apps {
    ($rest:literal) => {
        concat!(
            "SELECT id, name, target_id, spec, created_at, updated_at,
                EXISTS (SELECT 1 FROM app_secrets s WHERE s.app_id = apps.id
                    AND s.name = 'git_token') AS git_token_set
            FROM apps ",
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
    pub trigger: DeploymentTrigger,
    pub username: &'a str,
    pub spec: &'a AppSpec,
    pub env: &'a [StoredEnv],
    pub image_digest: Option<&'a str>,
    pub started_at: u64,
}

/// A deployment whose container serves its app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningDeployment {
    pub id: ShortId,
    pub app: ShortId,
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
        Ok(Self { pool })
    }

    /// Adds the local target unless it exists.
    pub async fn ensure_local_target(&self, now: u64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO targets (id, name, kind, created_at) VALUES (?, ?, 'local', ?)
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
        let rows = sqlx::query("SELECT id, name, kind, last_seen_at FROM targets ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(target_of).collect()
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
}

impl PlatformStore {
    pub async fn insert_deployment(&self, deployment: &NewDeployment<'_>) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO deployments (id, app_id, status, trigger, image_digest, spec, env,
                username, started_at)
            VALUES (?, ?, 'queued', ?, ?, ?, ?, ?, ?)",
        )
        .bind(deployment.id.to_string())
        .bind(deployment.app.to_string())
        .bind(deployment.trigger.as_str())
        .bind(deployment.image_digest)
        .bind(serde_json::to_string(deployment.spec)?)
        .bind(serde_json::to_vec(deployment.env)?)
        .bind(deployment.username)
        .bind(sql_int(deployment.started_at))
        .execute(&self.pool)
        .await?;
        Ok(())
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

    /// Ends the deployments that a stopped server left unfinished.
    pub async fn fail_unfinished(&self, now: u64) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE deployments SET status = 'failed', finished_at = ?,
                message = 'the server stopped during the deployment'
            WHERE status IN ('queued', 'building', 'deploying')",
        )
        .bind(sql_int(now))
        .execute(&self.pool)
        .await?;
        Ok(())
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
            "SELECT d.id, d.app_id, a.name, d.spec FROM deployments d
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
                    app_name: row.try_get::<&str, _>("name")?.parse()?,
                    spec: serde_json::from_str(row.try_get("spec")?)?,
                })
            })
            .collect()
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
    Ok(TargetEntry {
        id: id_of(row, "id")?,
        name: row.try_get("name")?,
        kind,
        last_seen_at: optional_time_of(row, "last_seen_at")?,
    })
}

fn app_of(row: &SqliteRow) -> anyhow::Result<AppEntry> {
    let spec: AppSpec = serde_json::from_str(row.try_get("spec")?)?;
    Ok(AppEntry {
        id: id_of(row, "id")?,
        name: row.try_get::<&str, _>("name")?.parse::<AppName>()?,
        target: id_of(row, "target_id")?,
        spec,
        git_token_set: row.try_get("git_token_set")?,
        created_at: time_of(row, "created_at")?,
        updated_at: time_of(row, "updated_at")?,
    })
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
                health_check_path: None,
                volumes: Vec::new(),
                restart: Default::default(),
                limits: Default::default(),
            },
            git_token_set: false,
            created_at: 10,
            updated_at: 10,
        }
    }

    #[tokio::test]
    async fn a_second_secret_value_replaces_the_first() -> anyhow::Result<()> {
        let (store, _path) = store().await?;
        let shop = app("abc-def", "shop");
        store.insert_app(&shop).await?;
        store.set_secret(shop.id, GIT_TOKEN, &[1]).await?;
        store.set_secret(shop.id, GIT_TOKEN, &[2]).await?;
        assert_eq!(store.secret(shop.id, GIT_TOKEN).await?, Some(vec![2]));
        assert!(store.app(shop.id).await?.context("app")?.git_token_set);
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
            trigger: DeploymentTrigger::Manual,
            username: "alice",
            spec: &app.spec,
            env,
            image_digest: None,
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
}
