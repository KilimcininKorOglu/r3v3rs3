//! Scheduled commands for apps stored in the SQLite database.

use super::{PlatformStore, id_of};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::AppScheduledCommand;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

impl PlatformStore {
    /// Lists all scheduled commands for a specific app.
    pub async fn list_scheduled_commands(
        &self,
        app_id: ShortId,
    ) -> anyhow::Result<Vec<AppScheduledCommand>> {
        let rows = sqlx::query(
            "SELECT id, app_id, name, schedule, command, enabled, last_run_at, last_status, last_output, created_at, updated_at
            FROM app_scheduled_commands
            WHERE app_id = ?
            ORDER BY created_at ASC",
        )
        .bind(app_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(scheduled_command_of).collect()
    }

    /// Lists all enabled scheduled commands across all apps.
    pub async fn list_all_enabled_scheduled_commands(
        &self,
    ) -> anyhow::Result<Vec<AppScheduledCommand>> {
        let rows = sqlx::query(
            "SELECT id, app_id, name, schedule, command, enabled, last_run_at, last_status, last_output, created_at, updated_at
            FROM app_scheduled_commands
            WHERE enabled = 1
            ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(scheduled_command_of).collect()
    }

    /// Retrieves a single scheduled command by its id.
    pub async fn scheduled_command(
        &self,
        id: ShortId,
    ) -> anyhow::Result<Option<AppScheduledCommand>> {
        let row = sqlx::query(
            "SELECT id, app_id, name, schedule, command, enabled, last_run_at, last_status, last_output, created_at, updated_at
            FROM app_scheduled_commands
            WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(scheduled_command_of).transpose()
    }

    /// Inserts a new scheduled command.
    pub async fn insert_scheduled_command(&self, cmd: &AppScheduledCommand) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO app_scheduled_commands
                (id, app_id, name, schedule, command, enabled, last_run_at, last_status, last_output, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(cmd.id.to_string())
        .bind(cmd.app_id.to_string())
        .bind(&cmd.name)
        .bind(&cmd.schedule)
        .bind(&cmd.command)
        .bind(if cmd.enabled { 1i64 } else { 0i64 })
        .bind(cmd.last_run_at)
        .bind(&cmd.last_status)
        .bind(&cmd.last_output)
        .bind(cmd.created_at)
        .bind(cmd.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Updates an existing scheduled command's configuration.
    pub async fn update_scheduled_command(&self, cmd: &AppScheduledCommand) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE app_scheduled_commands
            SET name = ?, schedule = ?, command = ?, enabled = ?, updated_at = ?
            WHERE id = ?",
        )
        .bind(&cmd.name)
        .bind(&cmd.schedule)
        .bind(&cmd.command)
        .bind(if cmd.enabled { 1i64 } else { 0i64 })
        .bind(cmd.updated_at)
        .bind(cmd.id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Deletes a scheduled command by its id.
    pub async fn delete_scheduled_command(&self, id: ShortId) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM app_scheduled_commands WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Records the execution outcome of a scheduled command.
    pub async fn record_scheduled_command_run(
        &self,
        id: ShortId,
        run_at: i64,
        status: &str,
        output: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE app_scheduled_commands
            SET last_run_at = ?, last_status = ?, last_output = ?, updated_at = ?
            WHERE id = ?",
        )
        .bind(run_at)
        .bind(status)
        .bind(output)
        .bind(run_at)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn scheduled_command_core(
    row: &SqliteRow,
) -> anyhow::Result<(ShortId, ShortId, String, String, String)> {
    Ok((
        id_of(row, "id")?,
        id_of(row, "app_id")?,
        row.try_get("name")?,
        row.try_get("schedule")?,
        row.try_get("command")?,
    ))
}

fn scheduled_command_of(row: &SqliteRow) -> anyhow::Result<AppScheduledCommand> {
    let (id, app_id, name, schedule, command) = scheduled_command_core(row)?;
    Ok(AppScheduledCommand {
        id,
        app_id,
        name,
        schedule,
        command,
        enabled: row.try_get::<i64, _>("enabled")? != 0,
        last_run_at: row.try_get("last_run_at")?,
        last_status: row.try_get("last_status")?,
        last_output: row.try_get("last_output")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
