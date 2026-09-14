//! A client for provider APIs that write the whole TXT record set of a name at once.

use super::{relative_name, DnsClient, TxtName, TxtRecord};
use async_trait::async_trait;

/// A provider API that reads and writes every TXT value of a name together.
#[async_trait]
pub trait RrsetApi: Send + Sync {
    /// A TXT value in the form of the API.
    type Value: Clone + PartialEq + Send + Sync;

    /// A challenge value in the form of the API.
    fn value(txt: &str) -> Self::Value;

    /// The name of the zone that holds `fqdn`, and the zone reference that the other methods take.
    async fn zone(&self, fqdn: &str) -> anyhow::Result<(String, String)>;

    /// The TXT values of `name`, relative to the zone. A missing record set has no values.
    async fn values(&self, zone: &str, name: &str) -> anyhow::Result<Vec<Self::Value>>;

    /// Replaces the TXT values of `name` with `values`.
    async fn put(&self, zone: &str, name: &str, values: &[Self::Value]) -> anyhow::Result<()>;

    async fn delete(&self, zone: &str, name: &str) -> anyhow::Result<()>;
}

/// A [`DnsClient`] that merges the challenge values into the record set of the name,
/// so the TXT values of other tools stay in place.
pub struct MergedRrset<T>(pub T);

/// A TXT value as a quoted string.
pub fn quoted(value: &str) -> String {
    format!("\"{value}\"")
}

#[async_trait]
impl<T: RrsetApi> DnsClient for MergedRrset<T> {
    async fn add_txt(&self, name: &TxtName) -> anyhow::Result<TxtRecord> {
        let (zone_name, zone) = self.0.zone(&name.fqdn).await?;
        let relative = relative_name(&name.fqdn, &zone_name).to_string();
        let mut values = self.0.values(&zone, &relative).await?;
        for value in name.values.iter().map(|value| T::value(value)) {
            if !values.contains(&value) {
                values.push(value);
            }
        }
        self.0.put(&zone, &relative, &values).await?;
        let mut record = TxtRecord::new(name, zone);
        record.ids = vec![relative];
        Ok(record)
    }

    async fn remove_txt(&self, record: &TxtRecord) -> anyhow::Result<()> {
        let Some(name) = record.ids.first() else {
            return Ok(());
        };
        let ours: Vec<T::Value> = record.values.iter().map(|value| T::value(value)).collect();
        let values = self.0.values(&record.zone, name).await?;
        let rest: Vec<T::Value> = values
            .iter()
            .filter(|value| !ours.contains(value))
            .cloned()
            .collect();
        if rest.len() == values.len() {
            return Ok(());
        }
        if rest.is_empty() {
            return self.0.delete(&record.zone, name).await;
        }
        self.0.put(&record.zone, name, &rest).await
    }
}
