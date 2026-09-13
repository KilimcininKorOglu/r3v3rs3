use super::{
    api::{ApiClient, ApiRequest},
    RecordApi, TXT_TTL,
};
use anyhow::anyhow;
use async_trait::async_trait;
use hyper::Method;
use serde_json::json;

pub const API_URL: &str = "https://api.cloudflare.com/client/v4";

pub struct Cloudflare {
    api: ApiClient,
    token: String,
}

impl Cloudflare {
    pub fn new(api: ApiClient, token: &str) -> Self {
        Self {
            api,
            token: token.to_string(),
        }
    }

    fn request(&self, method: Method, path: String) -> ApiRequest {
        ApiRequest::new(method, path).bearer(&self.token)
    }
}

#[async_trait]
impl RecordApi for Cloudflare {
    async fn zone(&self, fqdn: &str) -> anyhow::Result<String> {
        let lookup = |zone: &str| self.request(Method::GET, format!("/zones?name={zone}"));
        self.api
            .find_zone(fqdn, lookup, "/result/0/id")
            .await?
            .map(|(_, id)| id)
            .ok_or_else(|| anyhow!("no Cloudflare zone contains {fqdn}"))
    }

    async fn create(&self, zone: &str, fqdn: &str, value: &str) -> anyhow::Result<String> {
        let body = json!({ "type": "TXT", "name": fqdn, "content": value, "ttl": TXT_TTL });
        let request = self
            .request(Method::POST, format!("/zones/{zone}/dns_records"))
            .json(&body);
        self.api
            .json_id(request, "/result/id")
            .await?
            .ok_or_else(|| anyhow!("Cloudflare returned no record ID for {fqdn}"))
    }

    async fn delete(&self, zone: &str, id: &str) -> anyhow::Result<()> {
        let path = format!("/zones/{zone}/dns_records/{id}");
        self.api.send(self.request(Method::DELETE, path)).await?;
        Ok(())
    }
}
