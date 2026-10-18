//! Container registry credentials stored in the SQLite database.

use super::{PlatformStore, id_of, sql_int, time_of};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::RegistryEntry;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

fn registry_of(row: &SqliteRow) -> anyhow::Result<RegistryEntry> {
    Ok(RegistryEntry {
        id: id_of(row, "id")?,
        server: row.try_get("server")?,
        username: row.try_get("username")?,
        created_at: time_of(row, "created_at")?,
        updated_at: time_of(row, "updated_at")?,
    })
}

impl PlatformStore {
    /// Lists all container registries.
    pub async fn list_registries(&self) -> anyhow::Result<Vec<RegistryEntry>> {
        let rows = sqlx::query(
            "SELECT id, server, username, created_at, updated_at
            FROM registries
            ORDER BY server ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(registry_of).collect()
    }

    /// Retrieves a single registry by its ID.
    pub async fn registry(&self, id: ShortId) -> anyhow::Result<Option<RegistryEntry>> {
        let row = sqlx::query(
            "SELECT id, server, username, created_at, updated_at
            FROM registries
            WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(registry_of).transpose()
    }

    /// Retrieves a single registry by its server domain/address.
    pub async fn registry_by_server(&self, server: &str) -> anyhow::Result<Option<RegistryEntry>> {
        let row = sqlx::query(
            "SELECT id, server, username, created_at, updated_at
            FROM registries
            WHERE server = ?",
        )
        .bind(server)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(registry_of).transpose()
    }

    /// Reads the sealed secret of a registry by its ID.
    pub async fn registry_secret(&self, id: ShortId) -> anyhow::Result<Option<Vec<u8>>> {
        let row = sqlx::query("SELECT secret FROM registries WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get::<Vec<u8>, _>("secret")))
    }

    /// Inserts a new registry with its sealed secret.
    pub async fn insert_registry(
        &self,
        entry: &RegistryEntry,
        sealed: &[u8],
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO registries (id, server, username, secret, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(entry.id.to_string())
        .bind(&entry.server)
        .bind(&entry.username)
        .bind(sealed)
        .bind(sql_int(entry.created_at))
        .bind(sql_int(entry.updated_at))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Updates an existing registry. If sealed is Some, updates the secret as well.
    pub async fn update_registry(
        &self,
        entry: &RegistryEntry,
        sealed: Option<&[u8]>,
    ) -> anyhow::Result<()> {
        if let Some(secret) = sealed {
            sqlx::query(
                "UPDATE registries
                SET server = ?, username = ?, secret = ?, updated_at = ?
                WHERE id = ?",
            )
            .bind(&entry.server)
            .bind(&entry.username)
            .bind(secret)
            .bind(sql_int(entry.updated_at))
            .bind(entry.id.to_string())
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                "UPDATE registries
                SET server = ?, username = ?, updated_at = ?
                WHERE id = ?",
            )
            .bind(&entry.server)
            .bind(&entry.username)
            .bind(sql_int(entry.updated_at))
            .bind(entry.id.to_string())
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Deletes a registry by its ID.
    pub async fn delete_registry(&self, id: ShortId) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM registries WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Reads all registry secrets for key rotation: returns (id, sealed_secret).
    pub async fn all_registry_secrets(&self) -> anyhow::Result<Vec<(ShortId, Vec<u8>)>> {
        let rows = sqlx::query("SELECT id, secret FROM registries")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|r| {
                let id = id_of(&r, "id")?;
                let secret = r.get::<Vec<u8>, _>("secret");
                Ok((id, secret))
            })
            .collect()
    }

    /// Updates only the sealed secret of a registry (used during key rotation).
    pub async fn update_registry_secret(&self, id: ShortId, sealed: &[u8]) -> anyhow::Result<()> {
        sqlx::query("UPDATE registries SET secret = ? WHERE id = ?")
            .bind(sealed)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
