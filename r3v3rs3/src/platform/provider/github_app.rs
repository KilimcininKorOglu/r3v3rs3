//! The endpoints of a GitHub App: the conversion of its manifest, its installation and the tokens
//! of its installation.

use super::Site;
use crate::cdn::fetch::HttpClient;
use crate::certs::dns::api::ApiRequest;
use anyhow::{anyhow, bail};
use hyper::{Method, StatusCode};
use r3v3rs3_api::git_connection::GithubOwner;
use serde_json::Value;

/// The lifetime of an installation token. GitHub gives each token one hour.
const TOKEN_LIFETIME_MS: u64 = 3_600_000;

/// A GitHub App that GitHub created from a manifest.
pub(in crate::platform) struct Conversion {
    /// The page of the GitHub App.
    pub html_url: String,
    pub client_id: String,
    pub client_secret: String,
    /// The private key of the GitHub App in PEM.
    pub pem: String,
}

/// The client secret and the private key stay out of debug output and logs.
impl std::fmt::Debug for Conversion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conversion")
            .field("html_url", &self.html_url)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

/// The installation of a GitHub App on an account or an organization.
#[derive(Debug)]
pub(in crate::platform) struct Installation {
    pub id: u64,
    /// The account or the organization that installed the GitHub App.
    pub account: String,
}

/// A token that acts for an installation.
pub(in crate::platform) struct InstallationToken {
    pub token: String,
    /// The Unix time in milliseconds when the token expires.
    pub expires_at: u64,
}

#[derive(Debug)]
pub(in crate::platform) enum TokenError {
    /// The installation does not exist anymore.
    Gone,
    Failed(anyhow::Error),
}

impl Site<'_> {
    /// The page of GitHub that receives the manifest form. An organization owns the new GitHub App,
    /// or without one the account of the admin.
    pub fn manifest_form_url(
        &self,
        organization: Option<&GithubOwner>,
        state: &str,
    ) -> anyhow::Result<String> {
        let path = match organization {
            Some(organization) => format!("/organizations/{organization}/settings/apps/new"),
            None => "/settings/apps/new".to_string(),
        };
        let url = url::Url::parse_with_params(&format!("{}{path}", self.url), [("state", state)])?;
        Ok(url.into())
    }

    /// Trades the code of the manifest callback for the GitHub App. An error names no code,
    /// because an unused code still yields the private key.
    pub async fn convert_manifest(
        &self,
        http: &HttpClient,
        code: &str,
    ) -> anyhow::Result<Conversion> {
        let valid = !code.is_empty()
            && code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c));
        if !valid {
            bail!("invalid GitHub App manifest code");
        }
        let request = ApiRequest::new(Method::POST, format!("/app-manifests/{code}/conversions"));
        let app = self
            .api(http)?
            .json(request)
            .await
            .map_err(|err| anyhow!("{}", format!("{err:#}").replace(code, "<code>")))?;
        conversion_of(&app)
    }

    /// The installation of the GitHub App that signed `jwt`. A private GitHub App has at most one,
    /// on the account that owns it.
    pub async fn installation(
        &self,
        http: &HttpClient,
        jwt: &str,
    ) -> anyhow::Result<Option<Installation>> {
        let request = ApiRequest::new(Method::GET, "/app/installations".to_string()).bearer(jwt);
        let items = self.api(http)?.json(request).await?;
        Ok(items
            .as_array()
            .into_iter()
            .flatten()
            .find_map(installation_of))
    }

    /// A new token of an installation, which expires after one hour.
    pub async fn installation_token(
        &self,
        http: &HttpClient,
        jwt: &str,
        installation: u64,
        now: u64,
    ) -> Result<InstallationToken, TokenError> {
        let path = format!("/app/installations/{installation}/access_tokens");
        let request = ApiRequest::new(Method::POST, path).bearer(jwt);
        let api = self.api(http).map_err(TokenError::Failed)?;
        let (_, status, body) = api.exchange(request).await.map_err(TokenError::Failed)?;
        token_of(status, &body, now)
    }
}

fn conversion_of(app: &Value) -> anyhow::Result<Conversion> {
    let field = |name: &str| {
        app[name]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("GitHub returned no {name} for the new GitHub App"))
    };
    Ok(Conversion {
        html_url: field("html_url")?,
        client_id: field("client_id")?,
        client_secret: field("client_secret")?,
        pem: field("pem")?,
    })
}

fn installation_of(item: &Value) -> Option<Installation> {
    Some(Installation {
        id: item["id"].as_u64()?,
        account: item["account"]["login"].as_str()?.to_string(),
    })
}

/// A missing installation answers 404. The token lives one hour from `now`.
fn token_of(status: StatusCode, body: &[u8], now: u64) -> Result<InstallationToken, TokenError> {
    if status == StatusCode::NOT_FOUND {
        return Err(TokenError::Gone);
    }
    if !status.is_success() {
        return Err(TokenError::Failed(anyhow!(
            "GitHub answered {status} for an installation token"
        )));
    }
    let value =
        serde_json::from_slice::<Value>(body).map_err(|err| TokenError::Failed(err.into()))?;
    let token = value["token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| TokenError::Failed(anyhow!("GitHub returned no installation token")))?;
    Ok(InstallationToken {
        token: token.to_string(),
        expires_at: now.saturating_add(TOKEN_LIFETIME_MS),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::git_connection::{GitProvider, ProviderUrl};

    fn site(url: &ProviderUrl) -> Site<'_> {
        Site {
            provider: GitProvider::Github,
            url,
        }
    }

    #[test]
    fn the_manifest_form_goes_to_the_account_or_the_organization() -> anyhow::Result<()> {
        let github: ProviderUrl = "https://github.com".parse()?;
        assert_eq!(
            site(&github).manifest_form_url(None, "st")?,
            "https://github.com/settings/apps/new?state=st"
        );
        let own: ProviderUrl = "https://github.example.com".parse()?;
        let organization: GithubOwner = "my-team".parse()?;
        assert_eq!(
            site(&own).manifest_form_url(Some(&organization), "st")?,
            "https://github.example.com/organizations/my-team/settings/apps/new?state=st"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_manifest_code_is_traded_for_the_github_app() -> anyhow::Result<()> {
        let mut server = mockito::Server::new_async().await;
        let url: ProviderUrl = server.url().parse()?;
        let http = crate::cdn::fetch::build_client().await?;
        let conversion = server
            .mock("POST", "/api/v3/app-manifests/the-code/conversions")
            .with_status(201)
            .with_body(r#"{"id":1,"slug":"r3v3rs3-team","html_url":"https://github.com/apps/r3v3rs3-team","client_id":"Iv1.abc","client_secret":"s3cr3t","webhook_secret":null,"pem":"-----BEGIN RSA PRIVATE KEY-----"}"#)
            .create_async()
            .await;
        let app = site(&url).convert_manifest(&http, "the-code").await?;
        conversion.assert_async().await;
        assert_eq!(app.html_url, "https://github.com/apps/r3v3rs3-team");
        assert_eq!(app.client_id, "Iv1.abc");
        let debug = format!("{app:?}");
        assert!(
            !debug.contains("s3cr3t") && !debug.contains("PRIVATE"),
            "{debug}"
        );
        assert!(site(&url).convert_manifest(&http, "a/../b").await.is_err());

        server
            .mock("POST", "/api/v3/app-manifests/used-code/conversions")
            .with_status(404)
            .create_async()
            .await;
        let err = site(&url)
            .convert_manifest(&http, "used-code")
            .await
            .unwrap_err();
        assert!(!format!("{err:#}").contains("used-code"), "{err:#}");
        Ok(())
    }

    #[tokio::test]
    async fn the_installation_gives_the_tokens() -> anyhow::Result<()> {
        let mut server = mockito::Server::new_async().await;
        let url: ProviderUrl = server.url().parse()?;
        let http = crate::cdn::fetch::build_client().await?;
        server
            .mock("GET", "/api/v3/app/installations")
            .match_header("authorization", "Bearer the-jwt")
            .with_body(r#"[{"id":77,"account":{"login":"my-team"}}]"#)
            .create_async()
            .await;
        let installation = site(&url).installation(&http, "the-jwt").await?;
        let installation = installation.ok_or_else(|| anyhow!("no installation"))?;
        assert_eq!(
            (installation.id, installation.account.as_str()),
            (77, "my-team")
        );

        let token = server
            .mock("POST", "/api/v3/app/installations/77/access_tokens")
            .match_header("authorization", "Bearer the-jwt")
            .with_status(201)
            .with_body(r#"{"token":"ghs_abc","expires_at":"2026-09-24T12:00:00Z"}"#)
            .create_async()
            .await;
        let granted = site(&url)
            .installation_token(&http, "the-jwt", 77, 1_000)
            .await
            .map_err(|err| anyhow!("{err:?}"))?;
        token.assert_async().await;
        assert_eq!(granted.token, "ghs_abc");
        assert_eq!(granted.expires_at, 3_601_000);

        server
            .mock("POST", "/api/v3/app/installations/78/access_tokens")
            .with_status(404)
            .create_async()
            .await;
        let gone = site(&url).installation_token(&http, "the-jwt", 78, 0).await;
        assert!(matches!(gone, Err(TokenError::Gone)));
        Ok(())
    }
}
