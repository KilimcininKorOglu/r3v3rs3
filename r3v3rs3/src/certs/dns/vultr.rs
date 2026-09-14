use super::{
    api::{strings, ApiClient, ApiRequest},
    domain_zone, relative_name, RecordApi, TXT_TTL,
};
use anyhow::anyhow;
use async_trait::async_trait;
use hyper::Method;
use serde_json::json;

pub const API_URL: &str = "https://api.vultr.com/v2";
const PAGE_SIZE: usize = 100;

pub struct Vultr {
    api: ApiClient,
    token: String,
}

impl Vultr {
    pub fn new(api: ApiClient, token: &str) -> Self {
        Self {
            api,
            token: token.to_string(),
        }
    }

    fn request(&self, method: Method, path: String) -> ApiRequest {
        ApiRequest::new(method, path).bearer(&self.token)
    }

    /// Every domain of the account. The `next` link of a page is the cursor of the next page.
    async fn domains(&self) -> anyhow::Result<Vec<String>> {
        let mut domains = Vec::new();
        let mut path = format!("/domains?per_page={PAGE_SIZE}");
        loop {
            let response = self.api.json(self.request(Method::GET, path)).await?;
            domains.extend(strings(&response["domains"], "domain"));
            let next = response["meta"]["links"]["next"]
                .as_str()
                .unwrap_or_default();
            if next.is_empty() {
                break;
            }
            let cursor: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
            path = format!("/domains?per_page={PAGE_SIZE}&cursor={cursor}");
        }
        Ok(domains)
    }
}

#[async_trait]
impl RecordApi for Vultr {
    async fn zone(&self, fqdn: &str) -> anyhow::Result<String> {
        domain_zone(fqdn, &self.domains().await?, "Vultr")
    }

    async fn create(&self, zone: &str, fqdn: &str, value: &str) -> anyhow::Result<String> {
        // Vultr rejects TXT data without quotes.
        let body = json!({
            "type": "TXT",
            "name": relative_name(fqdn, zone),
            "data": format!("\"{value}\""),
            "ttl": TXT_TTL,
        });
        let request = self
            .request(Method::POST, format!("/domains/{zone}/records"))
            .json(&body);
        self.api
            .json_id(request, "/record/id")
            .await?
            .ok_or_else(|| anyhow!("Vultr returned no record ID for {fqdn}"))
    }

    async fn delete(&self, zone: &str, id: &str) -> anyhow::Result<()> {
        let path = format!("/domains/{zone}/records/{id}");
        self.api.send(self.request(Method::DELETE, path)).await?;
        Ok(())
    }
}
