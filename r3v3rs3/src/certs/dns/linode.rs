use super::{
    api::{id_text, ApiClient, ApiRequest},
    zone_with_id, RecordApi,
};
use anyhow::anyhow;
use async_trait::async_trait;
use hyper::Method;
use serde_json::{json, Value};

pub const API_URL: &str = "https://api.linode.com/v4";
/// The lowest TTL that Linode accepts, in seconds.
const TTL: u32 = 300;
const PAGE_SIZE: usize = 500;

pub struct Linode {
    api: ApiClient,
    token: String,
}

impl Linode {
    pub fn new(api: ApiClient, token: &str) -> Self {
        Self {
            api,
            token: token.to_string(),
        }
    }

    fn request(&self, method: Method, path: String) -> ApiRequest {
        ApiRequest::new(method, path).bearer(&self.token)
    }

    /// The name and the ID of every domain of the account.
    async fn domains(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut domains = Vec::new();
        for page in 1u64.. {
            let path = format!("/domains?page={page}&page_size={PAGE_SIZE}");
            let response = self.api.json(self.request(Method::GET, path)).await?;
            let items = response["data"].as_array().into_iter().flatten();
            domains.extend(items.filter_map(domain));
            if page >= response["pages"].as_u64().unwrap_or_default() {
                break;
            }
        }
        Ok(domains)
    }
}

fn domain(item: &Value) -> Option<(String, String)> {
    Some((item["domain"].as_str()?.to_string(), id_text(&item["id"])?))
}

#[async_trait]
impl RecordApi for Linode {
    async fn zone(&self, fqdn: &str) -> anyhow::Result<String> {
        let (_, id) = zone_with_id(fqdn, &self.domains().await?, "Linode")?;
        Ok(id)
    }

    async fn create(&self, zone: &str, fqdn: &str, value: &str) -> anyhow::Result<String> {
        let body = json!({ "type": "TXT", "name": fqdn, "target": value, "ttl_sec": TTL });
        let request = self
            .request(Method::POST, format!("/domains/{zone}/records"))
            .json(&body);
        self.api
            .json_id(request, "/id")
            .await?
            .ok_or_else(|| anyhow!("Linode returned no record ID for {fqdn}"))
    }

    async fn delete(&self, zone: &str, id: &str) -> anyhow::Result<()> {
        let path = format!("/domains/{zone}/records/{id}");
        self.api.send(self.request(Method::DELETE, path)).await?;
        Ok(())
    }
}
