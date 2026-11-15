//! Environment variable groups stored in the SQLite database.

use super::{PlatformStore, StoredEnv, id_of, sql_int, time_of};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::EnvGroupSummary;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEnvGroup {
    pub id: ShortId,
    pub name: String,
    pub description: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

fn env_group_of(row: &SqliteRow) -> anyhow::Result<StoredEnvGroup> {
    Ok(StoredEnvGroup {
        id: id_of(row, "id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        created_at: time_of(row, "created_at")?,
        updated_at: time_of(row, "updated_at")?,
    })
}

impl PlatformStore {
    /// Lists all environment variable groups with variable counts.
    pub async fn list_env_groups(&self) -> anyhow::Result<Vec<EnvGroupSummary>> {
        let rows = sqlx::query(
            "SELECT g.id, g.name, g.description, g.created_at, g.updated_at,
                    COUNT(v.key) as variable_count
            FROM env_groups g
            LEFT JOIN env_group_vars v ON g.id = v.group_id
            GROUP BY g.id
            ORDER BY g.name ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let count: i64 = row.try_get("variable_count")?;
                Ok(EnvGroupSummary {
                    id: id_of(row, "id")?,
                    name: row.try_get("name")?,
                    description: row.try_get("description")?,
                    variable_count: count as usize,
                    created_at: time_of(row, "created_at")?,
                    updated_at: time_of(row, "updated_at")?,
                })
            })
            .collect()
    }

    /// Retrieves an environment variable group by ID.
    pub async fn env_group(&self, id: ShortId) -> anyhow::Result<Option<StoredEnvGroup>> {
        let row = sqlx::query(
            "SELECT id, name, description, created_at, updated_at
            FROM env_groups
            WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(env_group_of).transpose()
    }

    /// Retrieves an environment variable group by name.
    pub async fn env_group_by_name(&self, name: &str) -> anyhow::Result<Option<StoredEnvGroup>> {
        let row = sqlx::query(
            "SELECT id, name, description, created_at, updated_at
            FROM env_groups
            WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(env_group_of).transpose()
    }

    /// Retrieves the environment variables of a group.
    pub async fn env_group_vars(&self, group_id: ShortId) -> anyhow::Result<Vec<StoredEnv>> {
        let rows = sqlx::query(
            "SELECT key, value, secret
            FROM env_group_vars
            WHERE group_id = ?
            ORDER BY key ASC",
        )
        .bind(group_id.to_string())
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

    /// Inserts a new environment variable group with its variables.
    pub async fn insert_env_group(
        &self,
        group: &StoredEnvGroup,
        vars: &[StoredEnv],
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO env_groups (id, name, description, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?)",
        )
        .bind(group.id.to_string())
        .bind(&group.name)
        .bind(&group.description)
        .bind(sql_int(group.created_at))
        .bind(sql_int(group.updated_at))
        .execute(&mut *tx)
        .await?;

        for var in vars {
            sqlx::query(
                "INSERT INTO env_group_vars (group_id, key, value, secret)
                VALUES (?, ?, ?, ?)",
            )
            .bind(group.id.to_string())
            .bind(&var.key)
            .bind(&var.sealed)
            .bind(var.secret)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Updates an environment variable group and replaces its variables.
    pub async fn update_env_group(
        &self,
        group: &StoredEnvGroup,
        vars: &[StoredEnv],
    ) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let res = sqlx::query(
            "UPDATE env_groups
            SET name = ?, description = ?, updated_at = ?
            WHERE id = ?",
        )
        .bind(&group.name)
        .bind(&group.description)
        .bind(sql_int(group.updated_at))
        .bind(group.id.to_string())
        .execute(&mut *tx)
        .await?;

        if res.rows_affected() == 0 {
            return Ok(false);
        }

        sqlx::query("DELETE FROM env_group_vars WHERE group_id = ?")
            .bind(group.id.to_string())
            .execute(&mut *tx)
            .await?;

        for var in vars {
            sqlx::query(
                "INSERT INTO env_group_vars (group_id, key, value, secret)
                VALUES (?, ?, ?, ?)",
            )
            .bind(group.id.to_string())
            .bind(&var.key)
            .bind(&var.sealed)
            .bind(var.secret)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(true)
    }

    /// Deletes an environment variable group and its variables.
    pub async fn delete_env_group(&self, id: ShortId) -> anyhow::Result<bool> {
        let res = sqlx::query("DELETE FROM env_groups WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}
