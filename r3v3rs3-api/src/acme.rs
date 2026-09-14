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
            DNS_01 => {
                let ready = self
                    .dns_provider
                    .as_ref()
                    .is_some_and(DnsProvider::has_credentials);
                if !ready {
                    return Err(Error::AcmeDnsProviderRequired);
                }
            }
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
/// `api_url` replaces the address of the provider API, for example with a test server.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum DnsProvider {
    Cloudflare {
        api_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
    },
    Route53 {
        access_key_id: String,
        secret_access_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
    },
    #[serde(rename = "digitalocean")]
    DigitalOcean {
        api_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
    },
    Hetzner {
        api_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
    },
}

impl DnsProvider {
    /// The `provider` value in the API and in the configuration.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Cloudflare { .. } => "cloudflare",
            Self::Route53 { .. } => "route53",
            Self::DigitalOcean { .. } => "digitalocean",
            Self::Hetzner { .. } => "hetzner",
        }
    }

    pub fn has_credentials(&self) -> bool {
        match self {
            Self::Cloudflare { api_token, .. }
            | Self::DigitalOcean { api_token, .. }
            | Self::Hetzner { api_token, .. } => !api_token.trim().is_empty(),
            Self::Route53 {
                access_key_id,
                secret_access_key,
                ..
            } => !access_key_id.trim().is_empty() && !secret_access_key.trim().is_empty(),
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
        Some(DnsProvider::Cloudflare {
            api_token: api_token.to_string(),
            api_url: None,
        })
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
    fn a_dns_provider_is_tagged_by_its_name() {
        let provider: DnsProvider =
            serde_json::from_str(r#"{"provider":"digitalocean","api_token":"secret-token"}"#)
                .unwrap();
        assert_eq!(provider.name(), "digitalocean");
        assert_eq!(
            serde_json::to_value(&provider).unwrap(),
            serde_json::json!({ "provider": "digitalocean", "api_token": "secret-token" })
        );
    }

    #[test]
    fn debug_output_does_not_contain_credentials() {
        let provider = DnsProvider::Route53 {
            access_key_id: "AKIDSECRET".to_string(),
            secret_access_key: "very-secret".to_string(),
            api_url: None,
        };
        let output = format!("{provider:?}");
        assert!(output.contains("route53"));
        assert!(!output.contains("AKIDSECRET"));
        assert!(!output.contains("very-secret"));
    }
}
