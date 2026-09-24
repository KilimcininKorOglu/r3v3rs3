//! The Git provider connections of the deployment platform. A connection is an OAuth application
//! of GitHub, GitLab or Gitea that lists the repositories of its account, clones them and installs
//! their webhooks.

use crate::container::AppName;
use crate::error::Error;
use crate::git::{checked_string, has_only_host_and_path};
use crate::id::ShortId;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use url::Url;
use utoipa::ToSchema;

/// The address of GitHub. A GitHub connection always uses it.
pub const GITHUB_URL: &str = "https://github.com";

/// The address of a GitLab connection without its own address.
pub const GITLAB_URL: &str = "https://gitlab.com";

/// The longest client id or client secret.
const MAX_CLIENT_VALUE_LENGTH: usize = 512;

/// The longest provider or public address.
const MAX_URL_LENGTH: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitProvider {
    Github,
    Gitlab,
    Gitea,
}

impl GitProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Gitea => "gitea",
        }
    }
}

impl FromStr for GitProvider {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "github" => Ok(Self::Github),
            "gitlab" => Ok(Self::Gitlab),
            "gitea" => Ok(Self::Gitea),
            other => Err(invalid(format!("unknown Git provider: {other}"))),
        }
    }
}

fn invalid(reason: String) -> Error {
    Error::InvalidGitConnection { reason }
}

checked_string!(
    /// The address of a Git provider: `https://`, or `http://` on a loopback address, without
    /// credentials, a query, a fragment or a trailing slash.
    ProviderUrl,
    check_provider_url
);

checked_string!(
    /// The public address of r3v3rs3 that a Git provider sends its webhook requests to:
    /// `https://` or `http://`, without credentials, a query, a fragment or a trailing slash.
    PublicUrl,
    check_public_url
);

fn check_provider_url(value: &str) -> Result<(), Error> {
    let valid = crate::acme::webhook_url_allowed(value) && is_plain_base_url(value);
    if valid {
        Ok(())
    } else {
        Err(invalid(format!(
            "the address must use https, or http on a loopback address, without a trailing slash: {value}"
        )))
    }
}

fn check_public_url(value: &str) -> Result<(), Error> {
    let http = Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"));
    if http && is_plain_base_url(value) {
        Ok(())
    } else {
        Err(Error::InvalidPublicUrl {
            url: value.to_string(),
        })
    }
}

/// Whether the value is a short URL with a host, without credentials, a query, a fragment or a
/// trailing slash.
fn is_plain_base_url(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    value.len() <= MAX_URL_LENGTH
        && url.host_str().is_some()
        && has_only_host_and_path(&url)
        && !value.ends_with('/')
}

/// A client id or a client secret: printable ASCII without spaces.
fn check_client_value(field: &str, value: &str) -> Result<(), Error> {
    let valid = !value.is_empty()
        && value.len() <= MAX_CLIENT_VALUE_LENGTH
        && value.chars().all(|c| c.is_ascii_graphic());
    if valid {
        Ok(())
    } else {
        Err(invalid(format!("the {field} is empty or invalid")))
    }
}

/// The body that creates or replaces a connection.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GitConnectionRequest {
    #[schema(value_type = String, example = "github-team")]
    pub name: AppName,
    pub provider: GitProvider,
    /// The address of the provider. GitHub uses `https://github.com`, and GitLab uses
    /// `https://gitlab.com` without it. A Gitea connection needs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "https://gitea.example.com")]
    pub url: Option<ProviderUrl>,
    /// The client id of the OAuth application.
    pub client_id: String,
    /// The client secret of the OAuth application. A new connection needs it. An update without
    /// it keeps the current secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

/// The client secret stays out of debug output and logs.
impl fmt::Debug for GitConnectionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitConnectionRequest")
            .field("name", &self.name)
            .field("provider", &self.provider)
            .field("url", &self.url)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl GitConnectionRequest {
    /// Checks the client values and returns the address of the provider.
    pub fn validate(&self) -> Result<ProviderUrl, Error> {
        check_client_value("client id", &self.client_id)?;
        if let Some(secret) = &self.client_secret {
            check_client_value("client secret", secret)?;
        }
        self.provider_url()
    }

    fn provider_url(&self) -> Result<ProviderUrl, Error> {
        match (self.provider, &self.url) {
            (GitProvider::Github, Some(url)) if url.as_str() != GITHUB_URL => Err(invalid(
                format!("a GitHub connection uses {GITHUB_URL}: {url}"),
            )),
            (GitProvider::Github, _) => GITHUB_URL.parse(),
            (GitProvider::Gitlab, None) => GITLAB_URL.parse(),
            (GitProvider::Gitea, None) => Err(invalid(
                "a Gitea connection needs the address of its server".to_string(),
            )),
            (_, Some(url)) => Ok(url.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    /// No account authorized the connection yet.
    NotConnected,
    Connected,
    /// The provider refused to renew the token. An admin must connect again.
    Expired,
}

/// A connection. The admin API never returns its client secret or its tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GitConnectionEntry {
    #[schema(value_type = String)]
    pub id: ShortId,
    #[schema(value_type = String)]
    pub name: AppName,
    pub provider: GitProvider,
    #[schema(value_type = String)]
    pub url: ProviderUrl,
    pub client_id: String,
    pub status: ConnectionStatus,
    /// The account of the provider that authorized the connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The Unix time in milliseconds.
    pub created_at: u64,
    /// The Unix time in milliseconds.
    pub updated_at: u64,
}

/// The settings of the deployment platform that the WebUI changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PlatformSettings {
    /// The address that the Git providers send the webhook requests of the apps to. Without it
    /// a connection installs no webhook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "https://deploy.example.com")]
    pub public_url: Option<PublicUrl>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(provider: GitProvider, url: Option<&str>) -> GitConnectionRequest {
        GitConnectionRequest {
            name: "team".parse().unwrap(),
            provider,
            url: url.map(|url| url.parse().unwrap()),
            client_id: "client".into(),
            client_secret: Some("s3cr3t".into()),
        }
    }

    #[test]
    fn each_provider_has_its_address() {
        let github = request(GitProvider::Github, None).validate().unwrap();
        assert_eq!(github.as_str(), GITHUB_URL);
        let gitlab = request(GitProvider::Gitlab, None).validate().unwrap();
        assert_eq!(gitlab.as_str(), GITLAB_URL);
        let own = Some("https://git.example.com");
        let gitlab = request(GitProvider::Gitlab, own).validate().unwrap();
        assert_eq!(gitlab.as_str(), "https://git.example.com");
        assert!(request(GitProvider::Gitea, None).validate().is_err());
        assert!(request(GitProvider::Github, own).validate().is_err());
    }

    #[test]
    fn a_provider_address_is_https_or_loopback_http() {
        for valid in [
            "https://git.example.com",
            "http://127.0.0.1:3000",
            "https://h.example/gitea",
        ] {
            assert!(valid.parse::<ProviderUrl>().is_ok(), "{valid}");
        }
        for invalid in [
            "http://git.example.com",
            "https://git.example.com/",
            "https://user:pass@git.example.com",
            "https://git.example.com?a=b",
            "ftp://git.example.com",
        ] {
            assert!(invalid.parse::<ProviderUrl>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn a_public_address_is_http_or_https_without_extras() {
        for valid in ["https://deploy.example.com", "http://203.0.113.5:8080"] {
            assert!(valid.parse::<PublicUrl>().is_ok(), "{valid}");
        }
        for invalid in [
            "deploy.example.com",
            "https://deploy.example.com/",
            "https://d.example#x",
        ] {
            assert!(invalid.parse::<PublicUrl>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn client_values_are_printable_words() {
        let mut spaced = request(GitProvider::Github, None);
        spaced.client_id = "a b".into();
        assert!(spaced.validate().is_err());
        let mut empty = request(GitProvider::Github, None);
        empty.client_secret = Some(String::new());
        assert!(empty.validate().is_err());
        let debug = format!("{:?}", request(GitProvider::Github, None));
        assert!(!debug.contains("s3cr3t"), "{debug}");
    }
}
