//! The audit log of the admin API. A single server keeps it in the log database. A cluster keeps it
//! in the cluster store, so every node reads the entries of every node.

use crate::clock::unix_ms;
use crate::log::open_database;
use r3v3rs3_api::audit::{
    AuditAction, AuditEntry, AuditQuery, DEFAULT_QUERY_LIMIT, MAX_QUERY_LIMIT, MAX_SUMMARY_LENGTH,
};
use sqlx::{Row, SqlitePool};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::error;

/// One day in milliseconds.
pub const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// The days of a query without `since`. The cluster store also reads at most these days before
/// `until`, because it lists the keys of each day separately.
pub const MAX_QUERY_DAYS: u64 = 31;

/// The longest username of an entry in bytes. A failed sign-in keeps the typed username.
const MAX_USERNAME_LENGTH: usize = 128;

/// The entries to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditFilter {
    /// The earliest time in Unix milliseconds.
    pub since: u64,
    /// The latest time in Unix milliseconds.
    pub until: u64,
    pub username: Option<String>,
    pub resource_id: Option<String>,
    pub limit: usize,
}

impl AuditFilter {
    /// The filter of an admin API query at `now` in Unix milliseconds.
    pub fn new(query: &AuditQuery, now: u64) -> Self {
        let until = query.until.unwrap_or(now);
        let since = until.saturating_sub(MAX_QUERY_DAYS * DAY_MS);
        let limit = query
            .limit
            .unwrap_or(DEFAULT_QUERY_LIMIT)
            .min(MAX_QUERY_LIMIT);
        Self {
            since: query.since.unwrap_or(since),
            until,
            username: query.username.clone(),
            resource_id: query.resource_id.clone(),
            limit: limit as usize,
        }
    }

    pub fn matches(&self, entry: &AuditEntry) -> bool {
        (self.since..=self.until).contains(&entry.time)
            && self
                .username
                .as_ref()
                .is_none_or(|username| *username == entry.username)
            && self
                .resource_id
                .as_ref()
                .is_none_or(|id| entry.resource_id.as_ref() == Some(id))
    }
}

#[async_trait::async_trait]
pub trait AuditStore: Send + Sync {
    async fn append(&self, entry: &AuditEntry) -> anyhow::Result<()>;

    /// The entries that the filter matches, newest first.
    async fn query(&self, filter: &AuditFilter) -> anyhow::Result<Vec<AuditEntry>>;

    /// Deletes the entries before the time in Unix milliseconds.
    async fn remove_before(&self, time: u64) -> anyhow::Result<()>;
}

/// An action to record. The audit log adds the account, the time and the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    pub action: AuditAction,
    pub resource_id: Option<String>,
    pub summary: String,
}

impl AuditRecord {
    pub fn new(action: AuditAction) -> Self {
        Self {
            action,
            resource_id: None,
            summary: String::new(),
        }
    }

    pub fn id(mut self, id: impl ToString) -> Self {
        self.resource_id = Some(id.to_string());
        self
    }

    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }
}

/// The audit log of a server.
pub struct AuditLog {
    store: Arc<dyn AuditStore>,
    /// The name of the cluster node. Empty without a cluster.
    node: String,
}

impl AuditLog {
    pub fn new(store: Arc<dyn AuditStore>, node: String) -> Self {
        Self { store, node }
    }

    /// Writes an entry in the background, so a slow store does not delay the server. A failed
    /// write is logged, and the recorded change stays.
    pub fn record(&self, username: &str, client: Option<IpAddr>, record: AuditRecord) {
        let entry = AuditEntry {
            time: unix_ms(),
            username: truncate(username, MAX_USERNAME_LENGTH),
            client,
            action: record.action,
            resource_id: record.resource_id,
            summary: truncate(&record.summary, MAX_SUMMARY_LENGTH),
            node: self.node.clone(),
        };
        let store = self.store.clone();
        tokio::spawn(async move {
            if let Err(err) = store.append(&entry).await {
                error!(action = ?entry.action, "failed to record the audit entry: {err:#}");
            }
        });
    }

    /// The entries that the filter matches, newest first.
    pub async fn query(&self, filter: &AuditFilter) -> anyhow::Result<Vec<AuditEntry>> {
        self.store.query(filter).await
    }

    pub async fn remove_before(&self, time: u64) -> anyhow::Result<()> {
        self.store.remove_before(time).await
    }
}

/// The longest start of `text` with at most `max` bytes that ends on a character boundary.
fn truncate(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// The audit log in the `audit_log` table of the log database.
pub struct SqliteAuditStore {
    path: PathBuf,
    pool: OnceCell<SqlitePool>,
}

impl SqliteAuditStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            pool: OnceCell::new(),
        }
    }

    /// Connects at the first use, so a server without an audited change creates no database.
    async fn pool(&self) -> anyhow::Result<&SqlitePool> {
        self.pool.get_or_try_init(|| connect(&self.path)).await
    }
}

async fn connect(path: &Path) -> anyhow::Result<SqlitePool> {
    let pool = open_database(path).await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS audit_log
        (
            time     INTEGER NOT NULL,
            username TEXT    NOT NULL,
            entry    TEXT    NOT NULL
        );",
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS audit_log_time ON audit_log (time)")
        .execute(&pool)
        .await?;
    Ok(pool)
}

/// SQLite stores signed 64-bit integers. A larger value is the largest integer.
fn sql_int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[async_trait::async_trait]
impl AuditStore for SqliteAuditStore {
    async fn append(&self, entry: &AuditEntry) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO audit_log (time, username, entry) VALUES (?, ?, ?)")
            .bind(sql_int(entry.time))
            .bind(&entry.username)
            .bind(serde_json::to_string(entry)?)
            .execute(self.pool().await?)
            .await?;
        Ok(())
    }

    async fn query(&self, filter: &AuditFilter) -> anyhow::Result<Vec<AuditEntry>> {
        let rows = sqlx::query(
            "SELECT entry FROM audit_log
            WHERE time BETWEEN ?1 AND ?2 AND (?3 IS NULL OR username = ?3)
            AND (?5 IS NULL OR json_extract(entry, '$.resource_id') = ?5)
            ORDER BY time DESC LIMIT ?4",
        )
        .bind(sql_int(filter.since))
        .bind(sql_int(filter.until))
        .bind(filter.username.as_deref())
        .bind(sql_int(filter.limit as u64))
        .bind(filter.resource_id.as_deref())
        .fetch_all(self.pool().await?)
        .await?;
        rows.iter()
            .map(|row| -> anyhow::Result<AuditEntry> {
                Ok(serde_json::from_str(row.try_get::<&str, _>(0)?)?)
            })
            .collect()
    }

    async fn remove_before(&self, time: u64) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM audit_log WHERE time < ?")
            .bind(sql_int(time))
            .execute(self.pool().await?)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(time: u64, username: &str) -> AuditEntry {
        AuditEntry {
            time,
            username: username.to_string(),
            client: None,
            action: AuditAction::AddProxy,
            resource_id: None,
            summary: String::new(),
            node: String::new(),
        }
    }

    fn times(entries: Vec<AuditEntry>) -> Vec<u64> {
        entries.iter().map(|entry| entry.time).collect()
    }

    #[test]
    fn a_query_without_a_period_covers_31_days_and_at_most_500_entries() {
        let now = 100 * DAY_MS;
        let filter = AuditFilter::new(&AuditQuery::default(), now);
        assert_eq!(
            (filter.since, filter.until, filter.limit),
            (69 * DAY_MS, now, 100)
        );
        let query = AuditQuery {
            since: Some(5),
            limit: Some(10_000),
            ..Default::default()
        };
        let filter = AuditFilter::new(&query, now);
        assert_eq!((filter.since, filter.limit), (5, 500));
    }

    #[test]
    fn a_long_text_is_cut_at_a_character_boundary() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("aç", 2), "a");
        assert_eq!(truncate("açb", 3), "aç");
    }

    #[tokio::test]
    async fn the_log_database_keeps_the_entries_until_the_retention() -> anyhow::Result<()> {
        let name = format!(
            "r3v3rs3-audit-{}.db",
            hex::encode(rand::random::<[u8; 8]>())
        );
        let path = std::env::temp_dir().join(name);
        let store = SqliteAuditStore::new(path.clone());
        store.append(&entry(1_000, "admin")).await?;
        store.append(&entry(9_000, "editor")).await?;
        store.append(&entry(5_000, "admin")).await?;

        let all = AuditFilter {
            since: 0,
            until: u64::MAX,
            username: None,
            resource_id: None,
            limit: 10,
        };
        assert_eq!(times(store.query(&all).await?), [9_000, 5_000, 1_000]);
        let admin = AuditFilter {
            username: Some("admin".into()),
            limit: 1,
            ..all.clone()
        };
        assert_eq!(times(store.query(&admin).await?), [5_000]);
        let web = AuditEntry {
            resource_id: Some("web".into()),
            ..entry(7_000, "editor")
        };
        store.append(&web).await?;
        let resource = AuditFilter {
            resource_id: Some("web".into()),
            ..all.clone()
        };
        assert_eq!(times(store.query(&resource).await?), [7_000]);

        store.remove_before(5_000).await?;
        assert_eq!(times(store.query(&all).await?), [9_000, 7_000, 5_000]);
        // Windows refuses to remove a file that an open connection holds.
        store.pool().await?.close().await;
        std::fs::remove_file(path)?;
        Ok(())
    }
}
