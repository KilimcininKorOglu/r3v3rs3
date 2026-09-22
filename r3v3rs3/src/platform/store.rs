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
];

/// One stored environment variable. The value is sealed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEnv {
    pub key: String,
    pub sealed: Vec<u8>,
    pub secret: bool,
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

    pub async fn has_target(&self, id: ShortId) -> anyhow::Result<bool> {
        let row = sqlx::query("SELECT 1 FROM targets WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    pub async fn apps(&self) -> anyhow::Result<Vec<AppEntry>> {
        let rows = sqlx::query(
            "SELECT id, name, target_id, spec, created_at, updated_at FROM apps ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(app_of).collect()
    }

    pub async fn app(&self, id: ShortId) -> anyhow::Result<Option<AppEntry>> {
        let row = sqlx::query(
            "SELECT id, name, target_id, spec, created_at, updated_at FROM apps WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(app_of).transpose()
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

/// Maps a unique constraint failure on the app name to `NameTaken`.
fn written(result: Result<Write, sqlx::Error>) -> anyhow::Result<Write> {
    match result {
        Err(sqlx::Error::Database(err))
            if err.is_unique_violation() && err.message().contains("apps.name") =>
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
            created_at: 10,
            updated_at: 10,
        }
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
