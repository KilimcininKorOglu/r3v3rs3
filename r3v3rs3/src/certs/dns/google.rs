use super::{
    TXT_TTL,
    api::{ApiClient, ApiRequest, TokenCache},
    rrset::{RrsetApi, quoted},
    zone_candidates,
};
use anyhow::{anyhow, bail};
use async_trait::async_trait;
use hyper::Method;
use ring::signature::RsaKeyPair;
use serde_json::{Value, json};
use time::OffsetDateTime;

pub const API_URL: &str = "https://dns.googleapis.com/dns/v1";
pub const AUTH_URL: &str = "https://oauth2.googleapis.com";
const SCOPE: &str = "https://www.googleapis.com/auth/ndev.clouddns.readwrite";
/// Seconds that a signed token request is valid. Google accepts at most one hour.
const ASSERTION_LIFETIME: i64 = 3600;

/// A service account from its JSON key file.
pub struct ServiceAccount {
    email: String,
    project_id: String,
    key: RsaKeyPair,
}

impl ServiceAccount {
    /// Reads the JSON key file of a service account. A `project_id` that is not empty replaces
    /// the project of the key. The error messages never contain the key.
    pub fn parse(key_file: &str, project_id: &str) -> anyhow::Result<Self> {
        let file: Value = serde_json::from_str(key_file)
            .map_err(|_| anyhow!("the Google Cloud service account key is not valid JSON"))?;
        let field = |name: &str| {
            file[name]
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| anyhow!("the Google Cloud service account key has no {name}"))
        };
        let email = field("client_email")?;
        let project_id = match project_id.trim() {
            "" => field("project_id")?,
            id => id.to_string(),
        };
        let pem = field("private_key")?;
        let der = rustls_pemfile::pkcs8_private_keys(&mut pem.as_bytes())
            .next()
            .and_then(Result::ok)
            .ok_or_else(|| anyhow!("the Google Cloud service account key has no PKCS#8 key"))?;
        let key = RsaKeyPair::from_pkcs8(der.secret_pkcs8_der())
            .map_err(|_| anyhow!("the Google Cloud service account key is not an RSA key"))?;
        Ok(Self {
            email,
            project_id,
            key,
        })
    }

    /// A JWT that asks the token endpoint `audience` for an access token, signed with RS256.
    fn assertion(&self, audience: &str) -> anyhow::Result<String> {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let claims = json!({
            "iss": self.email,
            "scope": SCOPE,
            "aud": audience,
            "iat": now,
            "exp": now + ASSERTION_LIFETIME,
        });
        crate::jwt::rs256(&self.key, &claims)
    }
}

/// Google Cloud DNS. The service account signs a token request for each access token.
pub struct GoogleCloud {
    api: ApiClient,
    auth: ApiClient,
    account: ServiceAccount,
    token: TokenCache,
}

/// The managed zone and the record set name of `name` in the zone reference `zone`.
/// A zone reference is the managed zone name and the DNS name of the zone, joined with `/`.
fn record_set(zone: &str, name: &str) -> anyhow::Result<(String, String)> {
    let (managed_zone, dns_name) = zone
        .split_once('/')
        .ok_or_else(|| anyhow!("invalid Google Cloud DNS zone reference {zone}"))?;
    Ok((managed_zone.to_string(), format!("{name}.{dns_name}.")))
}

impl GoogleCloud {
    pub fn new(api: ApiClient, auth: ApiClient, account: ServiceAccount) -> Self {
        Self {
            api,
            auth,
            account,
            token: TokenCache::default(),
        }
    }

    /// Requests a new access token with a signed JWT.
    async fn token_response(&self) -> anyhow::Result<Value> {
        let assertion = self.account.assertion(&self.auth.url("/token"))?;
        let request = ApiRequest::new(Method::POST, "/token".to_string()).form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &assertion),
        ]);
        self.auth.json(request).await
    }

    async fn request(&self, method: Method, path: String) -> anyhow::Result<ApiRequest> {
        let token = self.token.get(self.token_response()).await?;
        Ok(ApiRequest::new(method, path).bearer(&token))
    }

    fn zones_path(&self) -> String {
        format!("/projects/{}/managedZones", self.account.project_id)
    }

    /// The path of the TXT record set `name` in the zone reference `zone`.
    fn rrset_path(&self, zone: &str, name: &str) -> anyhow::Result<String> {
        let (managed_zone, rrset) = record_set(zone, name)?;
        Ok(format!(
            "{}/{managed_zone}/rrsets/{rrset}/TXT",
            self.zones_path()
        ))
    }

    /// The name of the managed zone for the DNS name `candidate` that the public internet can
    /// query. A private zone can have the same DNS name.
    async fn public_zone(&self, candidate: &str) -> anyhow::Result<Option<String>> {
        let path = format!("{}?dnsName={candidate}.", self.zones_path());
        let response = self
            .api
            .json(self.request(Method::GET, path).await?)
            .await?;
        let zones = response["managedZones"].as_array().into_iter().flatten();
        Ok(zones
            .filter(|zone| zone["visibility"].as_str() != Some("private"))
            .find_map(|zone| zone["name"].as_str())
            .map(str::to_string))
    }
}

#[async_trait]
impl RrsetApi for GoogleCloud {
    type Value = String;

    fn value(txt: &str) -> String {
        quoted(txt)
    }

    async fn zone(&self, fqdn: &str) -> anyhow::Result<(String, String)> {
        for candidate in zone_candidates(fqdn) {
            if let Some(managed_zone) = self.public_zone(candidate).await? {
                return Ok((candidate.to_string(), format!("{managed_zone}/{candidate}")));
            }
        }
        bail!("no public Google Cloud DNS zone contains {fqdn}")
    }

    async fn values(&self, zone: &str, name: &str) -> anyhow::Result<Vec<String>> {
        let request = self
            .request(Method::GET, self.rrset_path(zone, name)?)
            .await?;
        self.api.optional(request, "/rrdatas").await
    }

    async fn put(
        &self,
        zone: &str,
        name: &str,
        values: &[String],
        exists: bool,
    ) -> anyhow::Result<()> {
        let (managed_zone, rrset) = record_set(zone, name)?;
        let body = json!({ "name": rrset, "type": "TXT", "ttl": TXT_TTL, "rrdatas": values });
        // Google Cloud DNS creates a record set with a POST and changes it with a PATCH.
        let (method, path) = if exists {
            (Method::PATCH, self.rrset_path(zone, name)?)
        } else {
            let path = format!("{}/{managed_zone}/rrsets", self.zones_path());
            (Method::POST, path)
        };
        let request = self.request(method, path).await?.json(&body);
        self.api.send(request).await?;
        Ok(())
    }

    async fn delete(&self, zone: &str, name: &str) -> anyhow::Result<()> {
        let request = self
            .request(Method::DELETE, self.rrset_path(zone, name)?)
            .await?;
        self.api.send(request).await?;
        Ok(())
    }
}
