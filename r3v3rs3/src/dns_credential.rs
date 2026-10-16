//! The named DNS credentials that the ACME entries reference instead of carrying their own
//! provider. The stored credential holds the provider sealed with the key file of the config
//! directory, so the store holds no secret in plain text.

use crate::keys::crypto::ClusterKeys;
use base64::{Engine as _, engine::general_purpose};
use r3v3rs3_api::acme::{DnsCredentialInfo, DnsProvider};
use r3v3rs3_api::id::ShortId;
use serde_derive::{Deserialize, Serialize};

/// One stored DNS credential. The sealed value holds the provider with its secrets.
#[derive(Serialize, Deserialize)]
pub struct StoredDnsCredential {
    pub name: String,
    /// The `provider` name of the sealed credentials, in plain text for the lists.
    pub provider: String,
    /// The provider and its secrets, sealed with the key file of the config directory.
    pub sealed: String,
}

/// The associated data that binds a sealed credential to its id.
pub fn kv_key(id: ShortId) -> String {
    format!("dns-credential/{id}")
}

impl StoredDnsCredential {
    /// Seals the provider of the request into a stored credential.
    pub fn seal(
        id: ShortId,
        name: &str,
        provider: &DnsProvider,
        keys: &ClusterKeys,
    ) -> anyhow::Result<Self> {
        let json = serde_json::to_vec(provider)?;
        let sealed = keys.seal(&kv_key(id), &json)?;
        Ok(Self {
            name: name.to_string(),
            provider: provider.name().to_string(),
            sealed: general_purpose::STANDARD.encode(sealed),
        })
    }

    /// Opens the provider of the stored credential.
    pub fn open(&self, id: ShortId, keys: &ClusterKeys) -> anyhow::Result<DnsProvider> {
        let sealed = general_purpose::STANDARD
            .decode(&self.sealed)
            .context("the sealed dns credential is not base64")?;
        let json = keys.open(&kv_key(id), &sealed)?;
        Ok(serde_json::from_slice(&json)?)
    }

    /// The view of the store that carries no secret.
    pub fn info(&self, id: ShortId) -> DnsCredentialInfo {
        DnsCredentialInfo {
            id,
            name: self.name.clone(),
            provider: self.provider.clone(),
        }
    }
}

use anyhow::Context as _;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::crypto::ClusterKey;
    use r3v3rs3_api::acme::{TokenApi, TokenProvider};

    fn keys() -> ClusterKeys {
        ClusterKeys::new(vec![ClusterKey::new(&[7; 32]).unwrap()]).unwrap()
    }

    fn provider() -> DnsProvider {
        DnsProvider::Token(TokenProvider {
            provider: TokenApi::Cloudflare,
            api_token: "token".to_string(),
            api_url: None,
        })
    }

    #[test]
    fn a_sealed_credential_opens_to_the_same_provider() {
        let keys = keys();
        let id: ShortId = "abc123".parse().unwrap();
        let stored =
            StoredDnsCredential::seal(id, "Cloudflare - test", &provider(), &keys).unwrap();
        assert_eq!(stored.name, "Cloudflare - test");
        assert_eq!(stored.provider, "cloudflare");
        assert!(!stored.sealed.contains("token"));
        assert_eq!(stored.open(id, &keys).unwrap(), provider());
    }

    #[test]
    fn a_sealed_credential_does_not_open_under_another_id() {
        let keys = keys();
        let id: ShortId = "abc123".parse().unwrap();
        let stored = StoredDnsCredential::seal(id, "name", &provider(), &keys).unwrap();
        let other: ShortId = "other1".parse().unwrap();
        assert!(stored.open(other, &keys).is_err());
    }
}
