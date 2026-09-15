//! The audit log in the cluster store. Each node writes its entries below the key of the day, and
//! the leader deletes the days that passed the retention.

use super::storage::KvStorage;
use crate::audit::{AuditFilter, AuditStore, DAY_MS, MAX_QUERY_DAYS};
use crate::kv::KvItem;
use anyhow::Context as _;
use r3v3rs3_api::audit::AuditEntry;
use time::OffsetDateTime;
use tracing::warn;

#[async_trait::async_trait]
impl AuditStore for KvStorage {
    async fn append(&self, entry: &AuditEntry) -> anyhow::Result<()> {
        let day = day_of(entry.time).context("the time of the audit entry is out of range")?;
        let key = self.layout().audit_entry(&day, entry.time, rand::random());
        self.put_unconditional(key, &serde_json::to_vec(entry)?, None)
            .await
    }

    /// Reads the days from `until` back, at most `MAX_QUERY_DAYS` days.
    async fn query(&self, filter: &AuditFilter) -> anyhow::Result<Vec<AuditEntry>> {
        let mut entries = Vec::new();
        for day in query_days(filter) {
            let list = self.store().list(&self.layout().audit_day(&day)).await?;
            let matching = list
                .items
                .iter()
                .filter_map(|item| self.audit_entry(item))
                .filter(|entry| filter.matches(entry));
            entries.extend(matching);
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.time));
        entries.truncate(filter.limit);
        Ok(entries)
    }

    /// Deletes the days before the day of `time`. The entries of that day stay until the next day.
    async fn remove_before(&self, time: u64) -> anyhow::Result<()> {
        let cutoff = day_of(time).context("the retention time is out of range")?;
        let prefix = self.layout().audit();
        self.delete_stale(&prefix, |item| {
            entry_day(&prefix, &item.key).is_some_and(|day| day < cutoff.as_str())
        })
        .await
    }
}

impl KvStorage {
    /// The entry of an item. An entry that does not open is left out.
    fn audit_entry(&self, item: &KvItem) -> Option<AuditEntry> {
        self.decode_json(item)
            .inspect_err(|err| warn!(key = item.key, "invalid audit entry: {err:#}"))
            .ok()
    }
}

/// The UTC day of a time in Unix milliseconds, as `YYYY-MM-DD`. `None` after the year 9999.
fn day_of(time: u64) -> Option<String> {
    let seconds = i64::try_from(time / 1000).ok()?;
    let date = OffsetDateTime::from_unix_timestamp(seconds).ok()?.date();
    Some(format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    ))
}

/// The days of the filter from the newest, at most `MAX_QUERY_DAYS`.
fn query_days(filter: &AuditFilter) -> Vec<String> {
    let last = filter.until / DAY_MS;
    let first = (filter.since / DAY_MS).max(last.saturating_sub(MAX_QUERY_DAYS - 1));
    (first..=last)
        .rev()
        .filter_map(|day| day_of(day * DAY_MS))
        .collect()
}

/// The day segment of an entry key below `prefix`.
fn entry_day<'a>(prefix: &str, key: &'a str) -> Option<&'a str> {
    key.strip_prefix(prefix)?.split('/').next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_reads_at_most_31_days_from_the_newest() {
        assert_eq!(day_of(0).as_deref(), Some("1970-01-01"));
        assert_eq!(day_of(1_757_894_400_000).as_deref(), Some("2025-09-15"));

        let filter = |since, until| AuditFilter {
            since,
            until,
            username: None,
            resource_id: None,
            limit: 10,
        };
        assert_eq!(
            query_days(&filter(DAY_MS, 3 * DAY_MS + 5)),
            ["1970-01-04", "1970-01-03", "1970-01-02"]
        );
        assert_eq!(query_days(&filter(0, 100 * DAY_MS)).len(), 31);
        assert!(query_days(&filter(5 * DAY_MS, DAY_MS)).is_empty());
        assert_eq!(
            entry_day("p/audit/", "p/audit/1970-01-02/0000000000001-000000ff"),
            Some("1970-01-02")
        );
    }
}
