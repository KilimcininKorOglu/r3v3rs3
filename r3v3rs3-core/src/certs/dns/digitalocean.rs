use super::{
    RecordApi, TXT_TTL,
    api::{ApiClient, ApiRequest},
    domain_zone, relative_name,
};
use anyhow::anyhow;
use async_trait::async_trait;
use hyper::Method;
use serde_json::json;

pub const API_URL: &str = "https://api.digitalocean.com/v2";
const PAGE_SIZE: usize = 200;

pub struct DigitalOcean {
    api: ApiClient,
    token: String,
}

impl DigitalOcean {
    pub fn new(api: ApiClient, token: &str) -> Self {
        Self {
            api,
            token: token.to_string(),
        }
    }

    fn request(&self, method: Method, path: String) -> ApiRequest {
        ApiRequest::new(method, path).bearer(&self.token)
    }

    async fn domains(&self) -> anyhow::Result<Vec<String>> {
        let mut domains = Vec::new();
        for page in 1.. {
            let path = format!("/domains?per_page={PAGE_SIZE}&page={page}");
            let response = self.api.json(self.request(Method::GET, path)).await?;
            let items = response["domains"].as_array().cloned().unwrap_or_default();
            let count = items.len();
            domains.extend(
                items
                    .iter()
                    .filter_map(|item| item["name"].as_str().map(str::to_string)),
            );
            if count < PAGE_SIZE {
                break;
            }
        }
        Ok(domains)
    }
}

#[async_trait]
impl RecordApi for DigitalOcean {
    async fn zone(&self, fqdn: &str) -> anyhow::Result<String> {
        domain_zone(fqdn, &self.domains().await?, "DigitalOcean")
    }

    async fn create(&self, zone: &str, fqdn: &str, value: &str) -> anyhow::Result<String> {
        let name = relative_name(fqdn, zone);
        let body = json!({ "type": "TXT", "name": name, "data": value, "ttl": TXT_TTL });
        let request = self
            .request(Method::POST, format!("/domains/{zone}/records"))
            .json(&body);
        self.api
            .json_id(request, "/domain_record/id")
            .await?
            .ok_or_else(|| anyhow!("DigitalOcean returned no record ID for {fqdn}"))
    }

    async fn delete(&self, zone: &str, id: &str) -> anyhow::Result<()> {
        let path = format!("/domains/{zone}/records/{id}");
        self.api.send(self.request(Method::DELETE, path)).await?;
        Ok(())
    }
}
