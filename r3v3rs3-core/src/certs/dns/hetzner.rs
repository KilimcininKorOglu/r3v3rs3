use super::{
    DnsClient, TXT_TTL, TxtName, TxtRecord,
    api::{ApiClient, ApiRequest},
    relative_name,
};
use anyhow::anyhow;
use async_trait::async_trait;
use hyper::Method;
use serde_json::{Value, json};

pub const API_URL: &str = "https://api.hetzner.cloud/v1";

pub struct Hetzner {
    api: ApiClient,
    token: String,
}

impl Hetzner {
    pub fn new(api: ApiClient, token: &str) -> Self {
        Self {
            api,
            token: token.to_string(),
        }
    }

    fn request(&self, method: Method, path: String) -> ApiRequest {
        ApiRequest::new(method, path).bearer(&self.token)
    }

    /// The name and the ID of the zone that holds `fqdn`.
    async fn zone<'a>(&self, fqdn: &'a str) -> anyhow::Result<(&'a str, String)> {
        let lookup = |zone: &str| self.request(Method::GET, format!("/zones?name={zone}"));
        self.api
            .find_zone(fqdn, lookup, "/zones/0/id")
            .await?
            .ok_or_else(|| anyhow!("no Hetzner zone contains {fqdn}"))
    }

    async fn change(&self, record: &TxtRecord, action: &str, body: Value) -> anyhow::Result<()> {
        let path = format!(
            "/zones/{}/rrsets/{}/TXT/actions/{action}",
            record.zone,
            record.ids.first().map(String::as_str).unwrap_or_default()
        );
        self.api
            .send(self.request(Method::POST, path).json(&body))
            .await?;
        Ok(())
    }
}

/// The records of an RRSet request. A TXT value is a quoted string.
fn records(values: &[String]) -> Value {
    values
        .iter()
        .map(|value| json!({ "value": format!("\"{value}\"") }))
        .collect()
}

#[async_trait]
impl DnsClient for Hetzner {
    async fn add_txt(&self, name: &TxtName) -> anyhow::Result<TxtRecord> {
        let (zone, zone_id) = self.zone(&name.fqdn).await?;
        let mut record = TxtRecord::new(name, zone_id);
        // Hetzner addresses the RRSet by its name relative to the zone.
        record.ids = vec![relative_name(&name.fqdn, zone).to_string()];
        let body = json!({ "ttl": TXT_TTL, "records": records(&name.values) });
        self.change(&record, "add_records", body).await?;
        Ok(record)
    }

    async fn remove_txt(&self, record: &TxtRecord) -> anyhow::Result<()> {
        let body = json!({ "records": records(&record.values) });
        self.change(record, "remove_records", body).await
    }
}
