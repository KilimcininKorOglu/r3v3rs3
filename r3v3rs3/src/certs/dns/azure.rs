use super::{
    api::{ApiClient, ApiRequest, TokenCache},
    rrset::RrsetApi,
    zone_with_id, TXT_TTL,
};
use anyhow::anyhow;
use async_trait::async_trait;
use hyper::Method;
use serde_json::{json, Value};

pub const API_URL: &str = "https://management.azure.com";
pub const AUTH_URL: &str = "https://login.microsoftonline.com";
const API_VERSION: &str = "api-version=2018-05-01";

/// The credentials of a Microsoft Entra service principal.
pub struct Credentials {
    pub tenant_id: String,
    pub client_id: String,
    pub client_secret: String,
    pub subscription_id: String,
}

/// Azure DNS through Azure Resource Manager. The service principal gets its access tokens
/// with the client credentials grant.
pub struct Azure {
    api: ApiClient,
    auth: ApiClient,
    credentials: Credentials,
    token: TokenCache,
}

impl Azure {
    pub fn new(api: ApiClient, auth: ApiClient, credentials: Credentials) -> Self {
        Self {
            api,
            auth,
            credentials,
            token: TokenCache::default(),
        }
    }

    /// Requests a new access token with the client credentials grant.
    async fn token_response(&self) -> anyhow::Result<Value> {
        let tenant: String =
            url::form_urlencoded::byte_serialize(self.credentials.tenant_id.as_bytes()).collect();
        // The scope of Azure Resource Manager is its URL with the `.default` suffix.
        let scope = self.api.url("/.default");
        let request =
            ApiRequest::new(Method::POST, format!("/{tenant}/oauth2/v2.0/token")).form(&[
                ("grant_type", "client_credentials"),
                ("client_id", &self.credentials.client_id),
                ("client_secret", &self.credentials.client_secret),
                ("scope", &scope),
            ]);
        self.auth.json(request).await
    }

    async fn request(&self, method: Method, path: String) -> anyhow::Result<ApiRequest> {
        let token = self.token.get(self.token_response()).await?;
        Ok(ApiRequest::new(method, path).bearer(&token))
    }

    /// The name and the resource ID of every DNS zone of the subscription.
    async fn zones(&self) -> anyhow::Result<Vec<(String, String)>> {
        let subscription = &self.credentials.subscription_id;
        let mut path = format!(
            "/subscriptions/{subscription}/providers/Microsoft.Network/dnszones?{API_VERSION}"
        );
        let mut zones = Vec::new();
        loop {
            let response = self
                .api
                .json(self.request(Method::GET, path).await?)
                .await?;
            let items = response["value"].as_array().into_iter().flatten();
            zones.extend(items.filter_map(zone));
            let Some(next) = response["nextLink"]
                .as_str()
                .filter(|next| !next.is_empty())
            else {
                break;
            };
            // The next link is a full URL on the API host.
            path = next
                .strip_prefix(&self.api.url(""))
                .ok_or_else(|| anyhow!("Azure returned a next link on another host: {next}"))?
                .to_string();
        }
        Ok(zones)
    }

    fn record_path(zone: &str, name: &str) -> String {
        format!("{zone}/TXT/{name}?{API_VERSION}")
    }
}

fn zone(item: &Value) -> Option<(String, String)> {
    Some((
        item["name"].as_str()?.to_string(),
        item["id"].as_str()?.to_string(),
    ))
}

#[async_trait]
impl RrsetApi for Azure {
    /// The strings of one TXT record. Azure keeps the strings of a long value apart.
    type Value = Vec<String>;

    fn value(txt: &str) -> Vec<String> {
        vec![txt.to_string()]
    }

    async fn zone(&self, fqdn: &str) -> anyhow::Result<(String, String)> {
        zone_with_id(fqdn, &self.zones().await?, "Azure DNS")
    }

    async fn values(&self, zone: &str, name: &str) -> anyhow::Result<Vec<Vec<String>>> {
        let request = self
            .request(Method::GET, Self::record_path(zone, name))
            .await?;
        let records: Vec<Value> = self.api.optional(request, "/properties/TXTRecords").await?;
        records
            .iter()
            .map(|record| Ok(serde_json::from_value(record["value"].clone())?))
            .collect()
    }

    async fn put(
        &self,
        zone: &str,
        name: &str,
        values: &[Vec<String>],
        _exists: bool,
    ) -> anyhow::Result<()> {
        let records: Vec<Value> = values
            .iter()
            .map(|value| json!({ "value": value }))
            .collect();
        let body = json!({ "properties": { "TTL": TXT_TTL, "TXTRecords": records } });
        let request = self
            .request(Method::PUT, Self::record_path(zone, name))
            .await?
            .json(&body);
        self.api.send(request).await?;
        Ok(())
    }

    async fn delete(&self, zone: &str, name: &str) -> anyhow::Result<()> {
        let request = self
            .request(Method::DELETE, Self::record_path(zone, name))
            .await?;
        self.api.send(request).await?;
        Ok(())
    }
}
