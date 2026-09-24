//! The Git provider connections and the settings of the platform in the database.

use super::{PlatformStore, Write, id_of, optional_time_of, sql_int, time_of, written_name};
use anyhow::anyhow;
use r3v3rs3_api::git_connection::{ConnectionStatus, GitConnectionEntry, GitProvider, ProviderUrl};
use r3v3rs3_api::id::ShortId;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

/// The setting that holds the public address of r3v3rs3.
pub const PUBLIC_URL: &str = "public_url";

/// The query of the connection entries, followed by `$rest`. A literal, because sqlx takes only a
/// static query.
macro_rules! select_connections {
    ($rest:literal) => {
        concat!(
            "SELECT id, name, provider, url, client_id, status, account, created_at, updated_at
            FROM git_connections ",
            $rest
        )
    };
}

/// The sealed values of a connection, and the callback address of its authorization, which a
/// token renewal repeats.
pub struct StoredConnection {
    pub client_secret: Vec<u8>,
    pub access_token: Option<Vec<u8>>,
    pub refresh_token: Option<Vec<u8>>,
    /// The Unix time in milliseconds when the access token expires.
    pub expires_at: Option<u64>,
    pub redirect_uri: Option<String>,
}

/// The sealed tokens of an authorization.
pub struct StoredTokens<'a> {
    pub access_token: &'a [u8],
    pub refresh_token: Option<&'a [u8]>,
    /// The Unix time in milliseconds when the access token expires.
    pub expires_at: Option<u64>,
}

impl PlatformStore {
    pub async fn git_connections(&self) -> anyhow::Result<Vec<GitConnectionEntry>> {
        let rows = sqlx::query(select_connections!("ORDER BY name"))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(connection_of).collect()
    }

    pub async fn git_connection(&self, id: ShortId) -> anyhow::Result<Option<GitConnectionEntry>> {
        let row = sqlx::query(select_connections!("WHERE id = ?"))
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(connection_of).transpose()
    }

    /// The sealed values of a connection.
    pub async fn git_connection_secrets(
        &self,
        id: ShortId,
    ) -> anyhow::Result<Option<StoredConnection>> {
        let row = sqlx::query(
            "SELECT client_secret, access_token, refresh_token, expires_at, redirect_uri
            FROM git_connections WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(stored_connection_of).transpose()
    }

    pub async fn insert_git_connection(
        &self,
        entry: &GitConnectionEntry,
        client_secret: &[u8],
    ) -> anyhow::Result<Write> {
        let result = sqlx::query(
            "INSERT INTO git_connections (id, name, provider, url, client_id, client_secret,
                status, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, 'not_connected', ?, ?)",
        )
        .bind(entry.id.to_string())
        .bind(entry.name.as_str())
        .bind(entry.provider.as_str())
        .bind(entry.url.as_str())
        .bind(&entry.client_id)
        .bind(client_secret)
        .bind(sql_int(entry.created_at))
        .bind(sql_int(entry.updated_at))
        .execute(&self.pool)
        .await
        .map(|_| Write::Done);
        written_name(result, "git_connections.name")
    }

    /// Replaces the settings of a connection. A new provider, address or client id clears the
    /// tokens, because the old account authorized another application. Without a client secret
    /// the current secret stays.
    pub async fn update_git_connection(
        &self,
        entry: &GitConnectionEntry,
        client_secret: Option<&[u8]>,
    ) -> anyhow::Result<Write> {
        let result = sqlx::query(
            "UPDATE git_connections SET
                status = CASE WHEN provider = ?1 AND url = ?2 AND client_id = ?3
                    THEN status ELSE 'not_connected' END,
                account = CASE WHEN provider = ?1 AND url = ?2 AND client_id = ?3
                    THEN account END,
                access_token = CASE WHEN provider = ?1 AND url = ?2 AND client_id = ?3
                    THEN access_token END,
                refresh_token = CASE WHEN provider = ?1 AND url = ?2 AND client_id = ?3
                    THEN refresh_token END,
                expires_at = CASE WHEN provider = ?1 AND url = ?2 AND client_id = ?3
                    THEN expires_at END,
                redirect_uri = CASE WHEN provider = ?1 AND url = ?2 AND client_id = ?3
                    THEN redirect_uri END,
                provider = ?1, url = ?2, client_id = ?3,
                client_secret = COALESCE(?4, client_secret),
                name = ?5, updated_at = ?6
            WHERE id = ?7",
        )
        .bind(entry.provider.as_str())
        .bind(entry.url.as_str())
        .bind(&entry.client_id)
        .bind(client_secret)
        .bind(entry.name.as_str())
        .bind(sql_int(entry.updated_at))
        .bind(entry.id.to_string())
        .execute(&self.pool)
        .await
        .map(|done| {
            if done.rows_affected() == 1 {
                Write::Done
            } else {
                Write::NotFound
            }
        });
        written_name(result, "git_connections.name")
    }

    /// Stores the tokens of an authorized connection and marks it connected.
    pub async fn connect_git_connection(
        &self,
        id: ShortId,
        account: &str,
        redirect_uri: &str,
        tokens: &StoredTokens<'_>,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE git_connections SET status = 'connected', account = ?, redirect_uri = ?,
                access_token = ?, refresh_token = ?, expires_at = ? WHERE id = ?",
        )
        .bind(account)
        .bind(redirect_uri)
        .bind(tokens.access_token)
        .bind(tokens.refresh_token)
        .bind(tokens.expires_at.map(sql_int))
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Replaces the tokens of a connected connection after a renewal.
    pub async fn renew_git_tokens(
        &self,
        id: ShortId,
        tokens: &StoredTokens<'_>,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE git_connections SET access_token = ?, refresh_token = ?, expires_at = ?
            WHERE id = ? AND status = 'connected'",
        )
        .bind(tokens.access_token)
        .bind(tokens.refresh_token)
        .bind(tokens.expires_at.map(sql_int))
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Marks a connection expired and drops its tokens, because the provider refused them.
    pub async fn expire_git_connection(&self, id: ShortId) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE git_connections SET status = 'expired', access_token = NULL,
                refresh_token = NULL, expires_at = NULL WHERE id = ?",
        )
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Deletes a connection. Returns false when no connection has the id.
    pub async fn delete_git_connection(&self, id: ShortId) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM git_connections WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn setting(&self, key: &str) -> anyhow::Result<Option<String>> {
        let row = sqlx::query("SELECT value FROM platform_settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|row| row.try_get("value")).transpose()?)
    }

    /// Stores a setting, or deletes it without a value.
    pub async fn set_setting(&self, key: &str, value: Option<&str>) -> anyhow::Result<()> {
        let query = match value {
            Some(value) => sqlx::query(
                "INSERT INTO platform_settings (key, value) VALUES (?, ?)
                ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            )
            .bind(key)
            .bind(value),
            None => sqlx::query("DELETE FROM platform_settings WHERE key = ?").bind(key),
        };
        query.execute(&self.pool).await?;
        Ok(())
    }
}

fn stored_connection_of(row: &SqliteRow) -> anyhow::Result<StoredConnection> {
    Ok(StoredConnection {
        client_secret: row.try_get("client_secret")?,
        access_token: row.try_get("access_token")?,
        refresh_token: row.try_get("refresh_token")?,
        expires_at: optional_time_of(row, "expires_at")?,
        redirect_uri: row.try_get("redirect_uri")?,
    })
}

fn connection_of(row: &SqliteRow) -> anyhow::Result<GitConnectionEntry> {
    let (provider, url) = provider_of(row)?;
    let (status, account) = authorization_of(row)?;
    let (created_at, updated_at) = (time_of(row, "created_at")?, time_of(row, "updated_at")?);
    Ok(GitConnectionEntry {
        id: id_of(row, "id")?,
        name: row.try_get::<&str, _>("name")?.parse()?,
        provider,
        url,
        client_id: row.try_get("client_id")?,
        status,
        account,
        created_at,
        updated_at,
    })
}

fn authorization_of(row: &SqliteRow) -> anyhow::Result<(ConnectionStatus, Option<String>)> {
    Ok((status_of(row.try_get("status")?)?, row.try_get("account")?))
}

fn provider_of(row: &SqliteRow) -> anyhow::Result<(GitProvider, ProviderUrl)> {
    let provider = row.try_get::<&str, _>("provider")?.parse()?;
    let url = row.try_get::<&str, _>("url")?.parse()?;
    Ok((provider, url))
}

fn status_of(value: &str) -> anyhow::Result<ConnectionStatus> {
    match value {
        "not_connected" => Ok(ConnectionStatus::NotConnected),
        "connected" => Ok(ConnectionStatus::Connected),
        "expired" => Ok(ConnectionStatus::Expired),
        other => Err(anyhow!("unknown connection status {other}")),
    }
}
