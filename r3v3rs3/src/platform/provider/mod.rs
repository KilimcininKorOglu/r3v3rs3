//! The OAuth endpoints and the REST APIs of GitHub, GitLab and Gitea.

use crate::cdn::fetch::HttpClient;
use crate::certs::dns::api::{ApiClient, ApiRequest};
use anyhow::anyhow;
use hyper::{Method, StatusCode};
use r3v3rs3_api::git_connection::{GITHUB_URL, GitProvider, ProviderUrl};
use serde_json::Value;

mod repos;
pub(super) use repos::NewHook;

/// The REST API of github.com.
const GITHUB_API: &str = "https://api.github.com";

/// A Git provider at its address.
#[derive(Clone, Copy)]
pub(super) struct Site<'a> {
    pub provider: GitProvider,
    pub url: &'a ProviderUrl,
}

impl Site<'_> {
    /// The authorization page that the browser of the admin opens.
    pub fn authorize_url(&self, authorization: &Authorization<'_>) -> anyhow::Result<String> {
        let path = match self.provider {
            GitProvider::Github | GitProvider::Gitea => "/login/oauth/authorize",
            GitProvider::Gitlab => "/oauth/authorize",
        };
        let url = url::Url::parse_with_params(
            &format!("{}{path}", self.url),
            [
                ("client_id", authorization.client_id),
                ("redirect_uri", authorization.redirect_uri),
                ("response_type", "code"),
                ("scope", self.scopes()),
                ("state", authorization.state),
                ("code_challenge", authorization.challenge),
                ("code_challenge_method", "S256"),
            ],
        )?;
        Ok(url.into())
    }

    /// The scopes that list, clone and hook the repositories of the account.
    fn scopes(&self) -> &'static str {
        match self.provider {
            GitProvider::Github => "repo admin:repo_hook",
            GitProvider::Gitlab => "api",
            GitProvider::Gitea => "read:repository write:repository read:user",
        }
    }

    fn token_path(&self) -> &'static str {
        match self.provider {
            GitProvider::Github | GitProvider::Gitea => "/login/oauth/access_token",
            GitProvider::Gitlab => "/oauth/token",
        }
    }

    /// The base address of the REST API.
    fn api_url(&self) -> String {
        match self.provider {
            GitProvider::Github if self.url.as_str() == GITHUB_URL => GITHUB_API.to_string(),
            GitProvider::Github => format!("{}/api/v3", self.url),
            GitProvider::Gitlab => format!("{}/api/v4", self.url),
            GitProvider::Gitea => format!("{}/api/v1", self.url),
        }
    }

    /// A client of the REST API.
    pub fn api(&self, http: &HttpClient) -> anyhow::Result<ApiClient> {
        let url = self.api_url();
        ApiClient::new(http.clone(), Some(&url), &url)
    }

    /// Sends a form to the token endpoint and returns the granted tokens.
    pub async fn grant(
        &self,
        http: &HttpClient,
        form: &[(&str, &str)],
    ) -> Result<TokenGrant, GrantError> {
        let client = ApiClient::new(http.clone(), Some(self.url.as_str()), self.url.as_str())
            .map_err(GrantError::Failed)?;
        let request = ApiRequest::new(Method::POST, self.token_path().to_string())
            .header("accept", "application/json".to_string())
            .form(form);
        let (_, status, body) = client.exchange(request).await.map_err(GrantError::Failed)?;
        grant_of(status, &body)
    }

    /// The name of the account that owns the token.
    pub async fn account(&self, http: &HttpClient, token: &str) -> anyhow::Result<String> {
        let user = self
            .api(http)?
            .json(ApiRequest::new(Method::GET, "/user".to_string()).bearer(token))
            .await?;
        let field = match self.provider {
            GitProvider::Github | GitProvider::Gitea => "login",
            GitProvider::Gitlab => "username",
        };
        user[field]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("the provider returned no account name"))
    }
}

/// The values of an authorization request.
pub(super) struct Authorization<'a> {
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub state: &'a str,
    /// The S256 challenge of the PKCE verifier.
    pub challenge: &'a str,
}

/// The tokens of a token response.
pub(super) struct TokenGrant {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// The lifetime of the access token in seconds. GitHub gives no lifetime.
    pub expires_in: Option<u64>,
}

/// The tokens stay out of debug output and logs.
impl std::fmt::Debug for TokenGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenGrant")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(super) enum GrantError {
    /// The provider refused the code or the refresh token.
    Refused(String),
    /// The request failed before the provider decided.
    Failed(anyhow::Error),
}

impl From<GrantError> for anyhow::Error {
    fn from(err: GrantError) -> Self {
        match err {
            GrantError::Refused(reason) => anyhow!("the provider refused the grant: {reason}"),
            GrantError::Failed(err) => err,
        }
    }
}

/// GitHub answers a refused grant with 200 and an `error` field, the others with a 4xx status.
fn grant_of(status: StatusCode, body: &[u8]) -> Result<TokenGrant, GrantError> {
    let value = serde_json::from_slice::<Value>(body).ok();
    if let Some(error) = value.as_ref().and_then(|value| value["error"].as_str()) {
        let description = value
            .as_ref()
            .and_then(|value| value["error_description"].as_str());
        return Err(GrantError::Refused(
            description.unwrap_or(error).to_string(),
        ));
    }
    if status.is_client_error() {
        return Err(GrantError::Refused(format!(
            "the provider answered {status}"
        )));
    }
    match (status.is_success(), value) {
        (true, Some(value)) => token_grant(&value).map_err(GrantError::Failed),
        (true, None) => Err(GrantError::Failed(anyhow!(
            "the token response is not JSON"
        ))),
        (false, _) => Err(GrantError::Failed(anyhow!(
            "the token endpoint answered {status}"
        ))),
    }
}

fn token_grant(value: &Value) -> anyhow::Result<TokenGrant> {
    let access_token = value["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| anyhow!("the token response has no access token"))?;
    Ok(TokenGrant {
        access_token: access_token.to_string(),
        refresh_token: value["refresh_token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .map(str::to_string),
        expires_in: value["expires_in"].as_u64().filter(|seconds| *seconds > 0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(provider: GitProvider, url: &ProviderUrl) -> Site<'_> {
        Site { provider, url }
    }

    #[test]
    fn each_provider_has_its_endpoints() -> anyhow::Result<()> {
        let github: ProviderUrl = GITHUB_URL.parse()?;
        let own: ProviderUrl = "https://git.example.com".parse()?;
        assert_eq!(site(GitProvider::Github, &github).api_url(), GITHUB_API);
        assert_eq!(
            site(GitProvider::Gitlab, &own).api_url(),
            "https://git.example.com/api/v4"
        );
        assert_eq!(
            site(GitProvider::Gitea, &own).api_url(),
            "https://git.example.com/api/v1"
        );
        let authorization = Authorization {
            client_id: "id",
            redirect_uri: "https://r.example/oauth/git/callback",
            state: "st",
            challenge: "ch",
        };
        let url = site(GitProvider::Gitlab, &own).authorize_url(&authorization)?;
        assert!(
            url.starts_with("https://git.example.com/oauth/authorize?client_id=id&"),
            "{url}"
        );
        assert!(url.contains("redirect_uri=https%3A%2F%2Fr.example%2Foauth%2Fgit%2Fcallback"));
        assert!(url.contains("scope=api&state=st&code_challenge=ch&code_challenge_method=S256"));
        Ok(())
    }

    #[test]
    fn a_refused_grant_is_told_apart_from_a_failure() {
        let github_refusal =
            br#"{"error":"bad_verification_code","error_description":"The code is incorrect."}"#;
        match grant_of(StatusCode::OK, github_refusal) {
            Err(GrantError::Refused(reason)) => assert_eq!(reason, "The code is incorrect."),
            other => panic!("{other:?}"),
        }
        let gitlab_refusal = br#"{"error":"invalid_grant"}"#;
        assert!(matches!(
            grant_of(StatusCode::BAD_REQUEST, gitlab_refusal),
            Err(GrantError::Refused(_))
        ));
        assert!(matches!(
            grant_of(StatusCode::BAD_GATEWAY, b"down"),
            Err(GrantError::Failed(_))
        ));
        let granted = br#"{"access_token":"a","refresh_token":"r","expires_in":7200}"#;
        let grant = grant_of(StatusCode::OK, granted).unwrap();
        assert_eq!(grant.access_token, "a");
        assert_eq!(grant.refresh_token.as_deref(), Some("r"));
        assert_eq!(grant.expires_in, Some(7200));
        assert!(!format!("{grant:?}").contains("\"a\""));
    }
}
