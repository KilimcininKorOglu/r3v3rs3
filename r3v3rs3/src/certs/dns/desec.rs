use super::{
    api::{ApiClient, ApiRequest},
    rrset::RrsetApi,
};
use anyhow::anyhow;
use async_trait::async_trait;
use hyper::Method;
use serde_json::json;

pub const API_URL: &str = "https://desec.io/api/v1";
/// deSEC rejects a TTL below the minimum TTL of the domain. A domain accepts a TTL of 3600 seconds
/// unless the minimum TTL was raised.
const TTL: u32 = 3600;

pub struct Desec {
    api: ApiClient,
    token: String,
}

impl Desec {
    pub fn new(api: ApiClient, token: &str) -> Self {
        Self {
            api,
            token: token.to_string(),
        }
    }

    fn request(&self, method: Method, path: String) -> ApiRequest {
        ApiRequest::new(method, path).header("authorization", format!("Token {}", self.token))
    }

    fn rrset_path(zone: &str, name: &str) -> String {
        format!("/domains/{zone}/rrsets/{name}/TXT/")
    }
}

#[async_trait]
impl RrsetApi for Desec {
    /// deSEC returns the domain that is responsible for a name.
    async fn zone(&self, fqdn: &str) -> anyhow::Result<String> {
        let request = self.request(Method::GET, format!("/domains/?owns_qname={fqdn}"));
        self.api
            .json_id(request, "/0/name")
            .await?
            .ok_or_else(|| anyhow!("no deSEC domain contains {fqdn}"))
    }

    async fn values(&self, zone: &str, name: &str) -> anyhow::Result<Vec<String>> {
        let request = self.request(Method::GET, Self::rrset_path(zone, name));
        self.api.optional_strings(request, "records").await
    }

    async fn put(&self, zone: &str, name: &str, values: &[String]) -> anyhow::Result<()> {
        let body = json!({ "subname": name, "type": "TXT", "ttl": TTL, "records": values });
        let request = self
            .request(Method::PUT, Self::rrset_path(zone, name))
            .json(&body);
        self.api.send(request).await?;
        Ok(())
    }

    async fn delete(&self, zone: &str, name: &str) -> anyhow::Result<()> {
        let request = self.request(Method::DELETE, Self::rrset_path(zone, name));
        self.api.send(request).await?;
        Ok(())
    }
}
