use super::{
    DnsClient, TxtName, TxtRecord,
    api::{ApiClient, ApiRequest},
};
use anyhow::{Context, bail};
use async_trait::async_trait;
use hyper::Method;
use r3v3rs3_api::acme::webhook_url_allowed;
use serde_json::json;
use tracing::warn;

/// An HTTP service of the operator that receives the TXT records in a JSON `POST` request.
pub struct Webhook {
    api: ApiClient,
    token: String,
}

impl Webhook {
    /// `url` must use HTTPS, or HTTP on a loopback address.
    pub fn new(
        http: crate::cdn::fetch::HttpClient,
        url: &str,
        token: &str,
    ) -> anyhow::Result<Self> {
        if !webhook_url_allowed(url) {
            bail!("the DNS webhook URL must use https, or http on a loopback address");
        }
        Ok(Self {
            api: ApiClient::new(http, None, url.trim())?,
            token: token.trim().to_string(),
        })
    }

    async fn send(&self, action: &str, fqdn: &str, values: &[String]) -> anyhow::Result<()> {
        let body = json!({ "action": action, "fqdn": fqdn, "values": values });
        let mut request = ApiRequest::new(Method::POST, String::new()).json(&body);
        if !self.token.is_empty() {
            request = request.bearer(&self.token);
        }
        self.api
            .send(request)
            .await
            .with_context(|| format!("the DNS webhook failed to {action} {fqdn}"))?;
        Ok(())
    }
}

#[async_trait]
impl DnsClient for Webhook {
    async fn add_txt(&self, name: &TxtName) -> anyhow::Result<TxtRecord> {
        if let Err(err) = self.send("add", &name.fqdn, &name.values).await {
            // The service can have added some values before it failed.
            if let Err(remove_err) = self.send("remove", &name.fqdn, &name.values).await {
                warn!(fqdn = name.fqdn, err = %remove_err, "failed to remove TXT record");
            }
            return Err(err);
        }
        Ok(TxtRecord::new(name, String::new()))
    }

    async fn remove_txt(&self, record: &TxtRecord) -> anyhow::Result<()> {
        self.send("remove", &record.fqdn, &record.values).await
    }
}
