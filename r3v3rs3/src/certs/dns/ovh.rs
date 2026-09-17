use super::{
    RecordApi, TXT_TTL,
    api::{ApiClient, ApiRequest, id_text},
    domain_zone, relative_name,
};
use anyhow::anyhow;
use async_trait::async_trait;
use hyper::Method;
use once_cell::sync::OnceCell;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use time::OffsetDateTime;

/// OVHcloud signs each request with the application secret, the consumer key and a timestamp
/// of the OVH clock. A zone serves its changed records after a refresh.
pub struct Ovh {
    api: ApiClient,
    application_key: String,
    application_secret: String,
    consumer_key: String,
    /// The OVH clock minus the local clock, in seconds.
    clock_offset: OnceCell<i64>,
}

impl Ovh {
    pub fn new(
        api: ApiClient,
        application_key: &str,
        application_secret: &str,
        consumer_key: &str,
    ) -> Self {
        Self {
            api,
            application_key: application_key.to_string(),
            application_secret: application_secret.to_string(),
            consumer_key: consumer_key.to_string(),
            clock_offset: OnceCell::new(),
        }
    }

    /// The current time of the OVH clock. The first call reads the clock from `/auth/time`.
    async fn timestamp(&self) -> anyhow::Result<i64> {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        if let Some(offset) = self.clock_offset.get() {
            return Ok(now + offset);
        }
        let request = ApiRequest::new(Method::GET, "/auth/time".to_string());
        let response = self.api.json(request).await?;
        let server_time = response
            .as_i64()
            .ok_or_else(|| anyhow!("OVH returned no time: {response}"))?;
        self.clock_offset.get_or_init(|| server_time - now);
        Ok(server_time)
    }

    async fn call(
        &self,
        method: Method,
        path: String,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        let timestamp = self.timestamp().await?.to_string();
        let body = body.map(|body| body.to_string()).unwrap_or_default();
        let url = self.api.url(&path);
        let signed = [
            self.application_secret.as_str(),
            &self.consumer_key,
            method.as_str(),
            &url,
            &body,
            &timestamp,
        ];
        let mut request = ApiRequest::new(method.clone(), path)
            .header("x-ovh-application", self.application_key.clone())
            .header("x-ovh-consumer", self.consumer_key.clone())
            .header("x-ovh-signature", signature(&signed))
            .header("x-ovh-timestamp", timestamp);
        if !body.is_empty() {
            request = request.body("application/json", body.into_bytes());
        }
        self.api.json(request).await
    }
}

/// `$1$` and the SHA-1 hex digest of the parts joined with `+`.
fn signature(parts: &[&str]) -> String {
    format!("$1${}", hex::encode(Sha1::digest(parts.join("+"))))
}

#[async_trait]
impl RecordApi for Ovh {
    async fn zone(&self, fqdn: &str) -> anyhow::Result<String> {
        let response = self
            .call(Method::GET, "/domain/zone".to_string(), None)
            .await?;
        let zones: Vec<String> = serde_json::from_value(response)?;
        domain_zone(fqdn, &zones, "OVH")
    }

    async fn create(&self, zone: &str, fqdn: &str, value: &str) -> anyhow::Result<String> {
        let body = json!({
            "fieldType": "TXT",
            "subDomain": relative_name(fqdn, zone),
            "target": value,
            "ttl": TXT_TTL,
        });
        let path = format!("/domain/zone/{zone}/record");
        let response = self.call(Method::POST, path, Some(body)).await?;
        id_text(&response["id"]).ok_or_else(|| anyhow!("OVH returned no record ID for {fqdn}"))
    }

    async fn delete(&self, zone: &str, id: &str) -> anyhow::Result<()> {
        let path = format!("/domain/zone/{zone}/record/{id}");
        self.call(Method::DELETE, path, None).await?;
        Ok(())
    }

    async fn commit(&self, zone: &str) -> anyhow::Result<()> {
        let path = format!("/domain/zone/{zone}/refresh");
        self.call(Method::POST, path, None).await?;
        Ok(())
    }
}
