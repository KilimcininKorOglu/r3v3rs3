use super::{
    RecordApi,
    api::{ApiClient, ApiRequest, id_text, strings},
    domain_zone, relative_name,
};
use anyhow::{anyhow, bail};
use async_trait::async_trait;
use hyper::Method;
use serde_json::{Value, json};

pub const API_URL: &str = "https://api.porkbun.com/api/json/v3";
/// The largest number of domains in one page of `domain/listAll`.
const PAGE_SIZE: usize = 1000;

/// Porkbun takes the keys in the JSON body of every request. A record without a TTL gets the
/// lowest TTL of the account.
pub struct Porkbun {
    api: ApiClient,
    api_key: String,
    secret_api_key: String,
}

impl Porkbun {
    pub fn new(api: ApiClient, api_key: &str, secret_api_key: &str) -> Self {
        Self {
            api,
            api_key: api_key.to_string(),
            secret_api_key: secret_api_key.to_string(),
        }
    }

    /// Sends `body` with the keys and returns a response whose status is success.
    async fn call(&self, path: String, mut body: Value) -> anyhow::Result<Value> {
        body["apikey"] = self.api_key.clone().into();
        body["secretapikey"] = self.secret_api_key.clone().into();
        let request = ApiRequest::new(Method::POST, path.clone()).json(&body);
        let response = self.api.json(request).await?;
        let status = response["status"].as_str().unwrap_or_default();
        if !status.eq_ignore_ascii_case("success") {
            bail!("POST {path} returned {status}: {}", response["message"]);
        }
        Ok(response)
    }

    async fn domains(&self) -> anyhow::Result<Vec<String>> {
        let mut domains = Vec::new();
        for start in (0usize..).step_by(PAGE_SIZE) {
            let response = self
                .call("/domain/listAll".to_string(), json!({ "start": start }))
                .await?;
            let page = strings(&response["domains"], "domain");
            let count = page.len();
            domains.extend(page);
            if count < PAGE_SIZE {
                break;
            }
        }
        Ok(domains)
    }
}

#[async_trait]
impl RecordApi for Porkbun {
    async fn zone(&self, fqdn: &str) -> anyhow::Result<String> {
        domain_zone(fqdn, &self.domains().await?, "Porkbun")
    }

    async fn create(&self, zone: &str, fqdn: &str, value: &str) -> anyhow::Result<String> {
        let body = json!({ "name": relative_name(fqdn, zone), "type": "TXT", "content": value });
        let response = self.call(format!("/dns/create/{zone}"), body).await?;
        id_text(&response["id"]).ok_or_else(|| anyhow!("Porkbun returned no record ID for {fqdn}"))
    }

    async fn delete(&self, zone: &str, id: &str) -> anyhow::Result<()> {
        self.call(format!("/dns/delete/{zone}/{id}"), json!({}))
            .await?;
        Ok(())
    }
}
