use super::{
    api::{strings, ApiClient, ApiRequest},
    domain_zone,
    rrset::{quoted, RrsetApi},
};
use async_trait::async_trait;
use hyper::Method;
use serde_json::json;

pub const API_URL: &str = "https://api.gandi.net/v5/livedns";
const PAGE_SIZE: usize = 100;
/// The lowest TTL that Gandi LiveDNS accepts, in seconds.
const TTL: u32 = 300;

/// Gandi LiveDNS with a personal access token.
pub struct Gandi {
    api: ApiClient,
    token: String,
}

impl Gandi {
    pub fn new(api: ApiClient, token: &str) -> Self {
        Self {
            api,
            token: token.to_string(),
        }
    }

    fn request(&self, method: Method, path: String) -> ApiRequest {
        ApiRequest::new(method, path).bearer(&self.token)
    }

    fn record_path(zone: &str, name: &str) -> String {
        format!("/domains/{zone}/records/{name}/TXT")
    }

    /// Every domain of the account. A page after the last one is empty.
    async fn domains(&self) -> anyhow::Result<Vec<String>> {
        let mut domains = Vec::new();
        for page in 1.. {
            let path = format!("/domains?per_page={PAGE_SIZE}&page={page}");
            let response = self.api.json(self.request(Method::GET, path)).await?;
            let names = strings(&response, "fqdn");
            let last = names.len() < PAGE_SIZE;
            domains.extend(names);
            if last {
                break;
            }
        }
        Ok(domains)
    }
}

#[async_trait]
impl RrsetApi for Gandi {
    type Value = String;

    fn value(txt: &str) -> String {
        quoted(txt)
    }

    async fn zone(&self, fqdn: &str) -> anyhow::Result<(String, String)> {
        let zone = domain_zone(fqdn, &self.domains().await?, "Gandi")?;
        Ok((zone.clone(), zone))
    }

    async fn values(&self, zone: &str, name: &str) -> anyhow::Result<Vec<String>> {
        let request = self.request(Method::GET, Self::record_path(zone, name));
        self.api.optional(request, "/rrset_values").await
    }

    async fn put(&self, zone: &str, name: &str, values: &[String]) -> anyhow::Result<()> {
        let body = json!({ "rrset_ttl": TTL, "rrset_values": values });
        let request = self
            .request(Method::PUT, Self::record_path(zone, name))
            .json(&body);
        self.api.send(request).await?;
        Ok(())
    }

    async fn delete(&self, zone: &str, name: &str) -> anyhow::Result<()> {
        let request = self.request(Method::DELETE, Self::record_path(zone, name));
        self.api.send(request).await?;
        Ok(())
    }
}
