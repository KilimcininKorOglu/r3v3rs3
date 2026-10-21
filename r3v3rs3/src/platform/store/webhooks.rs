//! Outbound webhooks stored in the SQLite database.

use super::{PlatformStore, id_of, sql_int, time_of};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::webhook::{OutboundWebhook, WebhookEvent};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

#[derive(Debug, Clone)]
pub struct StoredWebhook {
    pub id: ShortId,
    pub name: String,
    pub url: String,
    pub has_secret: bool,
    pub enabled: bool,
    pub events: Vec<WebhookEvent>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl From<StoredWebhook> for OutboundWebhook {
    fn from(s: StoredWebhook) -> Self {
        Self {
            id: s.id,
            name: s.name,
            url: s.url,
            has_secret: s.has_secret,
            enabled: s.enabled,
            events: s.events,
            created_at: s.created_at,
            updated_at: s.updated_at,
        }
    }
}

fn parse_events(raw: &str) -> Vec<WebhookEvent> {
    raw.split(',')
        .filter_map(|s| s.trim().parse::<WebhookEvent>().ok())
        .collect()
}

pub(crate) fn format_events(events: &[WebhookEvent]) -> String {
    events
        .iter()
        .map(|e| e.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn webhook_of(row: &SqliteRow) -> anyhow::Result<StoredWebhook> {
    let secret_opt: Option<Vec<u8>> = row.try_get("secret")?;
    let events_str: String = row.try_get("events")?;
    let enabled_int: i64 = row.try_get("enabled")?;
    Ok(StoredWebhook {
        id: id_of(row, "id")?,
        name: row.try_get("name")?,
        url: row.try_get("url")?,
        has_secret: secret_opt.is_some(),
        enabled: enabled_int != 0,
        events: parse_events(&events_str),
        created_at: time_of(row, "created_at")?,
        updated_at: time_of(row, "updated_at")?,
    })
}

impl PlatformStore {
    /// Lists all outbound webhooks ordered by creation date descending.
    pub async fn list_webhooks(&self) -> anyhow::Result<Vec<StoredWebhook>> {
        let rows = sqlx::query(
            "SELECT id, name, url, secret, enabled, events, created_at, updated_at
            FROM outbound_webhooks
            ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(webhook_of).collect()
    }

    /// Retrieves an outbound webhook by ID.
    pub async fn webhook(&self, id: ShortId) -> anyhow::Result<Option<StoredWebhook>> {
        let row = sqlx::query(
            "SELECT id, name, url, secret, enabled, events, created_at, updated_at
            FROM outbound_webhooks
            WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(webhook_of).transpose()
    }

    /// Reads the sealed secret of an outbound webhook.
    pub async fn webhook_secret(&self, id: ShortId) -> anyhow::Result<Option<Vec<u8>>> {
        let row = sqlx::query("SELECT secret FROM outbound_webhooks WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|r| r.get::<Option<Vec<u8>>, _>("secret")))
    }

    /// Inserts a new outbound webhook with optional sealed secret.
    pub async fn insert_webhook(
        &self,
        entry: &StoredWebhook,
        sealed_secret: Option<&[u8]>,
    ) -> anyhow::Result<()> {
        let events_str = format_events(&entry.events);
        sqlx::query(
            "INSERT INTO outbound_webhooks (id, name, url, secret, enabled, events, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(entry.id.to_string())
        .bind(&entry.name)
        .bind(&entry.url)
        .bind(sealed_secret)
        .bind(if entry.enabled { 1i64 } else { 0i64 })
        .bind(events_str)
        .bind(sql_int(entry.created_at))
        .bind(sql_int(entry.updated_at))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Updates an existing outbound webhook.
    /// `sealed_secret`: `None` keeps existing, `Some(None)` clears, `Some(Some(bytes))` updates.
    pub async fn update_webhook(
        &self,
        entry: &StoredWebhook,
        sealed_secret: Option<Option<&[u8]>>,
    ) -> anyhow::Result<()> {
        let events_str = format_events(&entry.events);
        let enabled_val = if entry.enabled { 1i64 } else { 0i64 };
        let updated_at_val = sql_int(entry.updated_at);
        let id_str = entry.id.to_string();

        match sealed_secret {
            Some(secret) => {
                sqlx::query(
                    "UPDATE outbound_webhooks
                    SET name = ?, url = ?, secret = ?, enabled = ?, events = ?, updated_at = ?
                    WHERE id = ?",
                )
                .bind(&entry.name)
                .bind(&entry.url)
                .bind(secret)
                .bind(enabled_val)
                .bind(events_str)
                .bind(updated_at_val)
                .bind(id_str)
                .execute(&self.pool)
                .await?;
            }
            None => {
                sqlx::query(
                    "UPDATE outbound_webhooks
                    SET name = ?, url = ?, enabled = ?, events = ?, updated_at = ?
                    WHERE id = ?",
                )
                .bind(&entry.name)
                .bind(&entry.url)
                .bind(enabled_val)
                .bind(events_str)
                .bind(updated_at_val)
                .bind(id_str)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    /// Deletes an outbound webhook by ID.
    pub async fn delete_webhook(&self, id: ShortId) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM outbound_webhooks WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
