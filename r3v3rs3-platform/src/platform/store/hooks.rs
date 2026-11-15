//! The webhooks that the Git provider connections installed for the apps.

use super::{PlatformStore, id_of, optional_time_of, sql_int, time_of};
use r3v3rs3_api::container::AppName;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::AppHook;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

/// The webhook of an app at its provider. Without `remote_id` the installation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredHook {
    pub connection: ShortId,
    pub repository: String,
    pub remote_id: Option<String>,
    pub error: Option<String>,
    /// The JSON of the API error of a failed installation.
    pub failure: Option<String>,
    pub updated_at: u64,
}

impl PlatformStore {
    pub async fn app_hook(&self, app: ShortId) -> anyhow::Result<Option<StoredHook>> {
        let row = sqlx::query(
            "SELECT connection_id AS hook_connection, repository AS hook_repository,
                remote_id AS hook_remote_id, error AS hook_error, failure AS hook_failure,
                updated_at AS hook_updated_at
            FROM app_hooks WHERE app_id = ?",
        )
        .bind(app.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(stored_hook_of).transpose()
    }

    /// Stores the webhook of an app in place of its old one.
    pub async fn set_app_hook(&self, app: ShortId, hook: &StoredHook) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO app_hooks
                (app_id, connection_id, repository, remote_id, error, failure, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (app_id) DO UPDATE SET connection_id = excluded.connection_id,
                repository = excluded.repository, remote_id = excluded.remote_id,
                error = excluded.error, failure = excluded.failure,
                updated_at = excluded.updated_at",
        )
        .bind(app.to_string())
        .bind(hook.connection.to_string())
        .bind(&hook.repository)
        .bind(&hook.remote_id)
        .bind(&hook.error)
        .bind(&hook.failure)
        .bind(sql_int(hook.updated_at))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_app_hook(&self, app: ShortId) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM app_hooks WHERE app_id = ?")
            .bind(app.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The name of an app whose source uses the connection.
    pub async fn app_using_connection(
        &self,
        connection: ShortId,
    ) -> anyhow::Result<Option<AppName>> {
        let row = sqlx::query(
            "SELECT name FROM apps WHERE json_extract(spec, '$.source.connection') = ?
            ORDER BY name LIMIT 1",
        )
        .bind(connection.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| Ok(row.try_get::<&str, _>("name")?.parse()?))
            .transpose()
    }
}

fn stored_hook_of(row: &SqliteRow) -> anyhow::Result<StoredHook> {
    Ok(StoredHook {
        connection: id_of(row, "hook_connection")?,
        repository: row.try_get("hook_repository")?,
        remote_id: row.try_get("hook_remote_id")?,
        error: row.try_get("hook_error")?,
        failure: row.try_get("hook_failure")?,
        updated_at: time_of(row, "hook_updated_at")?,
    })
}

/// The webhook of an app row that joined its `app_hooks` row.
pub(super) fn app_hook_of(row: &SqliteRow) -> anyhow::Result<Option<AppHook>> {
    if optional_time_of(row, "hook_updated_at")?.is_none() {
        return Ok(None);
    }
    let hook = stored_hook_of(row)?;
    Ok(Some(AppHook {
        connection: hook.connection,
        repository: hook.repository,
        installed: hook.remote_id.is_some(),
        error: hook.error,
        failure: hook.failure,
        updated_at: hook.updated_at,
    }))
}
