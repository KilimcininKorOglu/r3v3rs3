use crate::{error::Error, id::ShortId, subject_name::SubjectName};
use base64::{engine::general_purpose, Engine as _};
use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

pub const HTTP_01: &str = "http-01";
pub const DNS_01: &str = "dns-01";
pub const TLS_ALPN_01: &str = "tls-alpn-01";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct Acme {
    #[schema(inline)]
    #[serde(flatten)]
    pub config: AcmeConfig,
    #[schema(value_type = [String], example = json!(["example.com", "*.example.com"]))]
    pub identifiers: Vec<SubjectName>,
    #[schema(value_type = String, example = "dns-01")]
    pub challenge_type: String,
    /// The DNS provider that publishes the TXT records of the `dns-01` challenge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_provider: Option<DnsProvider>,
}

impl Acme {
    /// Checks that the challenge can validate every identifier.
    pub fn validate(&self) -> Result<(), Error> {
        match self.challenge_type.as_str() {
            HTTP_01 | TLS_ALPN_01 => {}
            DNS_01 => match &self.dns_provider {
                Some(provider) if provider.has_credentials() => provider.validate()?,
                _ => return Err(Error::AcmeDnsProviderRequired),
            },
            _ => {
                return Err(Error::AcmeUnsupportedChallenge {
                    challenge: self.challenge_type.clone(),
                })
            }
        }
        if self.identifiers.is_empty() {
            return Err(Error::AcmeIdentifiersMissing);
        }
        self.identifiers
            .iter()
            .try_for_each(|identifier| self.validate_identifier(identifier))
    }

    fn validate_identifier(&self, identifier: &SubjectName) -> Result<(), Error> {
        match identifier {
            SubjectName::DnsName(name) if !name.contains('*') => Ok(()),
            SubjectName::WildcardDnsName(name) if !name.contains('*') => {
                if self.challenge_type == DNS_01 {
                    Ok(())
                } else {
                    Err(Error::AcmeWildcardNeedsDnsChallenge {
                        identifier: identifier.to_string(),
                    })
                }
            }
            _ => Err(Error::AcmeInvalidIdentifier {
                identifier: identifier.to_string(),
            }),
        }
    }
}

/// A DNS provider API that publishes the TXT records of the `dns-01` challenge.
///
/// The providers are grouped by the shape of their credentials. Every group is an object with
/// a `provider` name, so the groups do not change the serialized form.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(untagged)]
pub enum DnsProvider {
    Token(TokenProvider),
    Keyed(KeyedProvider),
    Cloud(CloudProvider),
    Local(LocalProvider),
}

/// A provider API that takes one API token.
///
/// `api_url` replaces the address of the provider API, for example with a test server.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct TokenProvider {
    pub provider: TokenApi,
    pub api_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TokenApi {
    Cloudflare,
    #[serde(rename = "digitalocean")]
    DigitalOcean,
    Hetzner,
    Linode,
    Vultr,
    Gandi,
    Desec,
}

impl TokenApi {
    pub fn name(self) -> &'static str {
        match self {
            Self::Cloudflare => "cloudflare",
            Self::DigitalOcean => "digitalocean",
            Self::Hetzner => "hetzner",
            Self::Linode => "linode",
            Self::Vultr => "vultr",
            Self::Gandi => "gandi",
            Self::Desec => "desec",
        }
    }
}

/// A provider API that takes a key pair or several keys.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum KeyedProvider {
    Route53 {
        access_key_id: String,
        secret_access_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
    },
    Porkbun {
        api_key: String,
        secret_api_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
    },
    Ovh {
        endpoint: OvhEndpoint,
        application_key: String,
        application_secret: String,
        consumer_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
    },
}

/// The API endpoint of an OVHcloud account, by its name in the OVH API clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OvhEndpoint {
    OvhEu,
    OvhCa,
    OvhUs,
    KimsufiEu,
    KimsufiCa,
    SoyoustartEu,
    SoyoustartCa,
}

impl OvhEndpoint {
    pub fn url(self) -> &'static str {
        match self {
            Self::OvhEu => "https://eu.api.ovh.com/1.0",
            Self::OvhCa => "https://ca.api.ovh.com/1.0",
            Self::OvhUs => "https://api.us.ovhcloud.com/1.0",
            Self::KimsufiEu => "https://eu.api.kimsufi.com/1.0",
            Self::KimsufiCa => "https://ca.api.kimsufi.com/1.0",
            Self::SoyoustartEu => "https://eu.api.soyoustart.com/1.0",
            Self::SoyoustartCa => "https://ca.api.soyoustart.com/1.0",
        }
    }
}

impl KeyedProvider {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Route53 { .. } => "route53",
            Self::Porkbun { .. } => "porkbun",
            Self::Ovh { .. } => "ovh",
        }
    }

    fn has_credentials(&self) -> bool {
        match self {
            Self::Route53 {
                access_key_id,
                secret_access_key,
                ..
            } => filled(&[access_key_id, secret_access_key]),
            Self::Porkbun {
                api_key,
                secret_api_key,
                ..
            } => filled(&[api_key, secret_api_key]),
            Self::Ovh {
                application_key,
                application_secret,
                consumer_key,
                ..
            } => filled(&[application_key, application_secret, consumer_key]),
        }
    }
}

/// A cloud platform API that authenticates with a service account of the platform.
///
/// `auth_url` replaces the address of the token endpoint, for example with a test server.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum CloudProvider {
    Azure {
        tenant_id: String,
        client_id: String,
        client_secret: String,
        subscription_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth_url: Option<String>,
    },
    GoogleCloud {
        /// The JSON key file of the service account.
        service_account_key: String,
        /// The project of the managed zones. An empty value selects the project of the key.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        project_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth_url: Option<String>,
    },
}

impl CloudProvider {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Azure { .. } => "azure",
            Self::GoogleCloud { .. } => "google_cloud",
        }
    }

    fn has_credentials(&self) -> bool {
        match self {
            Self::Azure {
                tenant_id,
                client_id,
                client_secret,
                subscription_id,
                ..
            } => filled(&[tenant_id, client_id, client_secret, subscription_id]),
            Self::GoogleCloud {
                service_account_key,
                ..
            } => filled(&[service_account_key]),
        }
    }
}

/// A provider that the operator runs, instead of the API of a DNS hosting service.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum LocalProvider {
    /// An HTTP service that receives the TXT records in a JSON `POST` request.
    Webhook {
        #[schema(example = "https://dns-hook.example.com/acme")]
        url: String,
        /// The bearer token of the requests. An empty token sends no `Authorization` header.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        token: String,
    },
    /// A program on the server host, from the `acme_exec` programs of `config.toml`.
    Exec {
        #[schema(example = "/usr/local/bin/r3v3rs3-dns-hook")]
        program: String,
    },
    /// A DNS server that accepts RFC 2136 dynamic updates signed with a TSIG key.
    Rfc2136 {
        /// The address of the primary server, as `host:port`.
        #[schema(example = "ns1.example.com:53")]
        server: String,
        /// The zone of the names. An empty value asks the server for the SOA record of each name.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        zone: String,
        key_name: String,
        #[serde(default)]
        key_algorithm: TsigKeyAlgorithm,
        /// The base64 secret of the TSIG key.
        key_secret: String,
    },
}

/// The HMAC algorithm of a TSIG key.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TsigKeyAlgorithm {
    #[default]
    HmacSha256,
    HmacSha384,
    HmacSha512,
}

/// Checks that `server` has a port and that `key_secret` is base64.
fn validate_rfc2136(server: &str, key_secret: &str) -> Result<(), Error> {
    use base64::Engine as _;

    let port = server
        .trim()
        .rsplit_once(':')
        .filter(|(host, _)| !host.is_empty())
        .and_then(|(_, port)| port.parse::<u16>().ok());
    if !matches!(port, Some(port) if port > 0) {
        return Err(Error::AcmeDnsProviderInvalid {
            field: "server".to_string(),
        });
    }
    let secret = base64::engine::general_purpose::STANDARD.decode(key_secret.trim());
    if !matches!(secret, Ok(key) if !key.is_empty()) {
        return Err(Error::AcmeDnsProviderInvalid {
            field: "key_secret".to_string(),
        });
    }
    Ok(())
}

impl LocalProvider {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Webhook { .. } => "webhook",
            Self::Exec { .. } => "exec",
            Self::Rfc2136 { .. } => "rfc2136",
        }
    }

    fn has_credentials(&self) -> bool {
        match self {
            Self::Webhook { url, .. } => filled(&[url]),
            Self::Exec { program } => filled(&[program]),
            Self::Rfc2136 {
                server,
                key_name,
                key_secret,
                ..
            } => filled(&[server, key_name, key_secret]),
        }
    }
}

/// Whether a webhook URL uses HTTPS, or HTTP on a loopback address.
pub fn webhook_url_allowed(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url.trim()) else {
        return false;
    };
    match (url.scheme(), url.host()) {
        ("https", Some(_)) => true,
        ("http", Some(url::Host::Domain(host))) => host.eq_ignore_ascii_case("localhost"),
        ("http", Some(url::Host::Ipv4(ip))) => ip.is_loopback(),
        ("http", Some(url::Host::Ipv6(ip))) => ip.is_loopback(),
        _ => false,
    }
}

/// Whether no credential is empty or only whitespace.
fn filled(values: &[&str]) -> bool {
    values.iter().all(|value| !value.trim().is_empty())
}

impl DnsProvider {
    /// The `provider` value in the API and in the configuration.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Token(provider) => provider.provider.name(),
            Self::Keyed(provider) => provider.name(),
            Self::Cloud(provider) => provider.name(),
            Self::Local(provider) => provider.name(),
        }
    }

    /// Checks the settings of the provider besides the presence of its credentials.
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Local(LocalProvider::Webhook { url, .. }) if !webhook_url_allowed(url) => {
                Err(Error::AcmeWebhookUrlInvalid { url: url.clone() })
            }
            Self::Local(LocalProvider::Rfc2136 {
                server, key_secret, ..
            }) => validate_rfc2136(server, key_secret),
            _ => Ok(()),
        }
    }

    pub fn has_credentials(&self) -> bool {
        match self {
            Self::Token(provider) => filled(&[&provider.api_token]),
            Self::Keyed(provider) => provider.has_credentials(),
            Self::Cloud(provider) => provider.has_credentials(),
            Self::Local(provider) => provider.has_credentials(),
        }
    }
}

impl fmt::Debug for DnsProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The credentials are secrets, so only the provider name is printed.
        f.debug_struct("DnsProvider")
            .field("provider", &self.name())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AcmeConfig {
    #[serde(default = "default_active", skip_serializing_if = "is_true")]
    pub active: bool,
    #[serde(default)]
    #[schema(example = "Let's Encrypt")]
    pub provider: String,
    #[schema(example = "60")]
    #[serde(default = "default_renewal_days")]
    pub renewal_days: u64,
}

fn default_active() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn default_renewal_days() -> u64 {
    60
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AcmeInfo {
    pub id: ShortId,
    #[schema(inline)]
    #[serde(flatten)]
    pub config: AcmeConfig,
    #[schema(example = json!(["example.com"]))]
    pub identifiers: Vec<String>,
    #[schema(value_type = String, example = "http-01")]
    pub challenge_type: String,
    /// The `provider` name of the DNS provider. The credentials are never returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "cloudflare")]
    pub dns_provider: Option<String>,
    pub next_renewal: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct AcmeRequest {
    #[schema(example = "https://acme-staging-v02.api.letsencrypt.org/directory")]
    pub server_url: String,
    #[schema(example = json!(["mailto:admin@example.com"]))]
    pub contacts: Vec<String>,
    #[serde(default)]
    pub eab: Option<ExternalAccountBinding>,
    #[schema(inline)]
    #[serde(flatten)]
    pub acme: Acme,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct ExternalAccountBinding {
    #[schema(example = "f9cf7e3faa1aca7e6086")]
    pub key_id: String,
    #[schema(value_type = String, example = "TszzWRgQWTUqo04dxmSuKDH06")]
    #[serde(
        serialize_with = "serialize_hmac_key",
        deserialize_with = "deserialize_hmac_key"
    )]
    pub hmac_key: Vec<u8>,
}

fn serialize_hmac_key<S>(hmac_key: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&general_purpose::URL_SAFE_NO_PAD.encode(hmac_key))
}

fn deserialize_hmac_key<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Deserialize;
    let hmac_key = String::deserialize(deserializer)?;
    general_purpose::URL_SAFE_NO_PAD
        .decode(hmac_key.as_bytes())
        .map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod test {
    use super::*;

    fn acme(identifiers: &[&str], challenge_type: &str) -> Acme {
        Acme {
            config: AcmeConfig::default(),
            identifiers: identifiers.iter().map(|id| id.parse().unwrap()).collect(),
            challenge_type: challenge_type.to_string(),
            dns_provider: None,
        }
    }

    fn cloudflare(api_token: &str) -> Option<DnsProvider> {
        Some(DnsProvider::Token(TokenProvider {
            provider: TokenApi::Cloudflare,
            api_token: api_token.to_string(),
            api_url: None,
        }))
    }

    #[test]
    fn http_01_accepts_plain_domain_names() {
        assert!(acme(&["example.com", "www.example.com"], HTTP_01)
            .validate()
            .is_ok());
    }

    #[test]
    fn http_01_rejects_a_wildcard_domain_name() {
        let result = acme(&["example.com", "*.example.com"], HTTP_01).validate();
        assert!(matches!(
            result,
            Err(Error::AcmeWildcardNeedsDnsChallenge { identifier }) if identifier == "*.example.com"
        ));
    }

    #[test]
    fn tls_alpn_01_accepts_plain_domain_names_but_not_a_wildcard() {
        assert!(acme(&["example.com"], TLS_ALPN_01).validate().is_ok());
        // RFC 8737 section 3 forbids a wildcard identifier for this challenge.
        let result = acme(&["example.com", "*.example.com"], TLS_ALPN_01).validate();
        assert!(matches!(
            result,
            Err(Error::AcmeWildcardNeedsDnsChallenge { identifier }) if identifier == "*.example.com"
        ));
    }

    #[test]
    fn dns_01_accepts_a_wildcard_domain_name() {
        let mut request = acme(&["example.com", "*.example.com"], DNS_01);
        request.dns_provider = cloudflare("token");
        assert!(request.validate().is_ok());
    }

    #[test]
    fn dns_01_needs_a_provider_with_credentials() {
        let mut request = acme(&["example.com"], DNS_01);
        assert!(matches!(
            request.validate(),
            Err(Error::AcmeDnsProviderRequired)
        ));
        request.dns_provider = cloudflare(" ");
        assert!(matches!(
            request.validate(),
            Err(Error::AcmeDnsProviderRequired)
        ));
    }

    #[test]
    fn ip_addresses_and_inner_asterisks_are_rejected() {
        for name in ["127.0.0.1", "a.*.example.com"] {
            assert!(matches!(
                acme(&[name], HTTP_01).validate(),
                Err(Error::AcmeInvalidIdentifier { .. })
            ));
        }
    }

    #[test]
    fn an_empty_list_and_an_unknown_challenge_are_rejected() {
        assert!(matches!(
            acme(&[], HTTP_01).validate(),
            Err(Error::AcmeIdentifiersMissing)
        ));
        assert!(matches!(
            acme(&["example.com"], "tls-sni-01").validate(),
            Err(Error::AcmeUnsupportedChallenge { .. })
        ));
    }

    #[test]
    fn every_dns_provider_keeps_its_serialized_form() {
        let providers = [
            serde_json::json!({ "provider": "cloudflare", "api_token": "t" }),
            serde_json::json!({ "provider": "digitalocean", "api_token": "t", "api_url": "http://u" }),
            serde_json::json!({ "provider": "hetzner", "api_token": "t" }),
            serde_json::json!({ "provider": "linode", "api_token": "t" }),
            serde_json::json!({ "provider": "vultr", "api_token": "t" }),
            serde_json::json!({ "provider": "gandi", "api_token": "t" }),
            serde_json::json!({ "provider": "desec", "api_token": "t" }),
            serde_json::json!({ "provider": "porkbun", "api_key": "k", "secret_api_key": "s" }),
            serde_json::json!({
                "provider": "ovh",
                "endpoint": "soyoustart-ca",
                "application_key": "a",
                "application_secret": "s",
                "consumer_key": "c",
            }),
            serde_json::json!({ "provider": "route53", "access_key_id": "a", "secret_access_key": "s" }),
            serde_json::json!({
                "provider": "azure",
                "tenant_id": "t",
                "client_id": "c",
                "client_secret": "s",
                "subscription_id": "u",
                "auth_url": "http://a",
            }),
            serde_json::json!({ "provider": "google_cloud", "service_account_key": "{}" }),
            serde_json::json!({ "provider": "webhook", "url": "https://h" }),
            serde_json::json!({ "provider": "webhook", "url": "https://h", "token": "t" }),
            serde_json::json!({ "provider": "exec", "program": "/usr/local/bin/hook" }),
            serde_json::json!({ "provider": "rfc2136", "server": "ns:53", "key_name": "k", "key_algorithm": "hmac-sha512", "key_secret": "c2VjcmV0" }),
            serde_json::json!({ "provider": "rfc2136", "server": "ns:53", "zone": "example.com", "key_name": "k", "key_algorithm": "hmac-sha256", "key_secret": "c2VjcmV0" }),
            serde_json::json!({ "provider": "google_cloud", "service_account_key": "{}", "project_id": "p" }),
        ];
        for value in providers {
            let provider: DnsProvider = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(provider.name(), value["provider"]);
            assert!(provider.has_credentials());
            assert_eq!(serde_json::to_value(&provider).unwrap(), value);
        }
    }

    #[test]
    fn a_webhook_url_needs_https_unless_it_is_a_loopback_address() {
        for url in [
            "https://dns-hook.example.com/acme",
            " https://10.0.0.5:8443 ",
            "http://127.0.0.1:8080/dns",
            "http://[::1]/dns",
            "http://LocalHost/dns",
        ] {
            assert!(webhook_url_allowed(url), "{url}");
        }
        for url in [
            "http://dns-hook.example.com/acme",
            "http://10.0.0.5/dns",
            "ftp://127.0.0.1/dns",
            "not a url",
            "",
        ] {
            assert!(!webhook_url_allowed(url), "{url}");
        }

        let mut request = acme(&["example.com"], DNS_01);
        request.dns_provider = Some(DnsProvider::Local(LocalProvider::Webhook {
            url: "http://dns-hook.example.com/acme".to_string(),
            token: String::new(),
        }));
        assert!(matches!(
            request.validate(),
            Err(Error::AcmeWebhookUrlInvalid { url }) if url == "http://dns-hook.example.com/acme"
        ));
    }

    #[test]
    fn rfc2136_needs_a_server_port_and_a_base64_secret() {
        let provider = |server: &str, key_secret: &str| {
            DnsProvider::Local(LocalProvider::Rfc2136 {
                server: server.to_string(),
                zone: String::new(),
                key_name: "r3v3rs3".to_string(),
                key_algorithm: TsigKeyAlgorithm::HmacSha256,
                key_secret: key_secret.to_string(),
            })
        };
        for server in ["ns1.example.com:53", "[::1]:5353", "192.0.2.1:53"] {
            assert!(provider(server, "c2VjcmV0").validate().is_ok(), "{server}");
        }
        for (server, key_secret, field) in [
            ("ns1.example.com", "c2VjcmV0", "server"),
            (":53", "c2VjcmV0", "server"),
            ("ns1.example.com:0", "c2VjcmV0", "server"),
            ("ns1.example.com:53", "not base64!", "key_secret"),
        ] {
            assert!(
                matches!(
                    provider(server, key_secret).validate(),
                    Err(Error::AcmeDnsProviderInvalid { field: invalid }) if invalid == field
                ),
                "{server} {key_secret}"
            );
        }
    }

    #[test]
    fn an_unknown_dns_provider_is_rejected() {
        let result = serde_json::from_str::<DnsProvider>(r#"{"provider":"other","api_token":"t"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn a_keyed_provider_needs_every_key() {
        let provider = DnsProvider::Keyed(KeyedProvider::Route53 {
            access_key_id: "AKID".to_string(),
            secret_access_key: " ".to_string(),
            api_url: None,
        });
        assert!(!provider.has_credentials());
    }

    #[test]
    fn debug_output_does_not_contain_credentials() {
        let provider = DnsProvider::Keyed(KeyedProvider::Route53 {
            access_key_id: "AKIDSECRET".to_string(),
            secret_access_key: "very-secret".to_string(),
            api_url: None,
        });
        let output = format!("{provider:?}");
        assert!(output.contains("route53"));
        assert!(!output.contains("AKIDSECRET"));
        assert!(!output.contains("very-secret"));
    }
}
