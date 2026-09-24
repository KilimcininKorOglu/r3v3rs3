//! The OAuth authorization of the Git provider connections. An admin starts an authorization, the
//! provider sends the browser back to the public callback with a code, and the platform trades the
//! code for tokens. A token that expires is renewed with its refresh token before it is used.

use super::connections::connection_aad;
use super::provider::{Authorization, GrantError, Site, TokenGrant};
use super::store::StoredTokens;
use super::{Platform, not_found};
use crate::cdn::fetch::HttpClient;
use crate::clock::unix_ms;
use anyhow::Context as _;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::git_connection::{
    AuthorizeResponse, ConnectionStatus, GitConnectionEntry, RedirectUri,
};
use r3v3rs3_api::id::ShortId;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tracing::warn;

/// How long an authorization waits for the callback of the provider.
const AUTHORIZATION_LIFETIME: Duration = Duration::from_secs(600);

/// The most authorizations that wait at the same time. A new one drops the oldest.
const MAX_PENDING: usize = 64;

/// A token is renewed when it expires within this many milliseconds.
const RENEWAL_MARGIN_MS: u64 = 60_000;

/// An authorization that waits for the callback of the provider.
struct Pending {
    connection: ShortId,
    redirect_uri: RedirectUri,
    /// The PKCE verifier, which only the token request reveals.
    verifier: String,
    /// The account of r3v3rs3 that started the authorization.
    username: String,
    client: Option<IpAddr>,
    created: Instant,
}

/// The waiting authorizations by their `state`. Each state is used once.
#[derive(Default)]
pub(super) struct PendingAuthorizations(std::sync::Mutex<HashMap<String, Pending>>);

impl PendingAuthorizations {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        match self.0.lock() {
            Ok(pending) => pending,
            // The map stays valid when a holder panics.
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn insert(&self, state: String, pending: Pending) {
        let mut map = self.lock();
        map.retain(|_, pending| pending.created.elapsed() < AUTHORIZATION_LIFETIME);
        if map.len() >= MAX_PENDING
            && let Some(oldest) = map
                .iter()
                .min_by_key(|(_, pending)| pending.created)
                .map(|(state, _)| state.clone())
        {
            map.remove(&oldest);
        }
        map.insert(state, pending);
    }

    /// Removes the authorization of `state`. An expired authorization is not returned.
    fn take(&self, state: &str) -> Option<Pending> {
        self.lock()
            .remove(state)
            .filter(|pending| pending.created.elapsed() < AUTHORIZATION_LIFETIME)
    }
}

/// A connection that an account of the provider authorized.
#[derive(Debug)]
pub struct Authorized {
    pub entry: GitConnectionEntry,
    /// The account of r3v3rs3 that started the authorization.
    pub username: String,
    pub client: Option<IpAddr>,
}

/// 32 random bytes in URL-safe base64.
fn random_word() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

/// The S256 challenge of a PKCE verifier.
fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Whether a token that expires at `expires_at` must be renewed before its use.
fn needs_renewal(expires_at: Option<u64>, now: u64) -> bool {
    expires_at.is_some_and(|at| now.saturating_add(RENEWAL_MARGIN_MS) >= at)
}

pub(super) fn provider_failed(err: impl Into<anyhow::Error>) -> anyhow::Error {
    Error::GitProviderFailed {
        reason: format!("{:#}", err.into()),
    }
    .into()
}

impl Platform {
    /// The HTTPS client of the provider APIs, built at its first use.
    pub(super) async fn http(&self) -> anyhow::Result<&HttpClient> {
        self.http
            .get_or_try_init(crate::cdn::fetch::build_client)
            .await
    }

    /// Starts an authorization and returns the page of the provider that the admin opens.
    pub async fn authorize_git_connection(
        &self,
        id: ShortId,
        redirect_uri: &RedirectUri,
        username: &str,
        client: Option<IpAddr>,
    ) -> anyhow::Result<AuthorizeResponse> {
        let entry = self.git_connection(id).await?;
        let (state, verifier) = (random_word(), random_word());
        let authorization = Authorization {
            client_id: &entry.client_id,
            redirect_uri: redirect_uri.as_str(),
            state: &state,
            challenge: &challenge_of(&verifier),
        };
        let url = site(&entry).authorize_url(&authorization)?;
        let pending = Pending {
            connection: id,
            redirect_uri: redirect_uri.clone(),
            verifier,
            username: username.to_string(),
            client,
            created: Instant::now(),
        };
        self.pending.insert(state, pending);
        Ok(AuthorizeResponse { url })
    }

    /// Drops the authorization of `state`, because the provider reported an error.
    pub fn cancel_git_authorization(&self, state: &str) -> Option<ShortId> {
        self.pending.take(state).map(|pending| pending.connection)
    }

    /// Trades the code of the callback for the tokens of the connection.
    pub async fn finish_git_authorization(
        &self,
        state: &str,
        code: &str,
    ) -> anyhow::Result<Authorized> {
        let pending = self.pending.take(state).ok_or(Error::OauthStateInvalid)?;
        let entry = self.git_connection(pending.connection).await?;
        let (grant, account) = self.trade_code(&entry, &pending, code).await?;
        let tokens = self.seal_grant(entry.id, &grant, None)?;
        let redirect_uri = pending.redirect_uri.as_str();
        let stored = tokens.stored();
        if !self
            .store
            .connect_git_connection(entry.id, &account, redirect_uri, &stored)
            .await?
        {
            return Err(not_found(entry.id));
        }
        Ok(Authorized {
            entry: self.git_connection(entry.id).await?,
            username: pending.username,
            client: pending.client,
        })
    }

    /// Trades the code for tokens, and returns them with the account that owns them.
    async fn trade_code(
        &self,
        entry: &GitConnectionEntry,
        pending: &Pending,
        code: &str,
    ) -> anyhow::Result<(TokenGrant, String)> {
        let secret = self.client_secret(entry.id).await?;
        let form = [
            ("client_id", entry.client_id.as_str()),
            ("client_secret", secret.as_str()),
            ("code", code),
            ("grant_type", "authorization_code"),
            ("redirect_uri", pending.redirect_uri.as_str()),
            ("code_verifier", pending.verifier.as_str()),
        ];
        let http = self.http().await?;
        let grant = site(entry)
            .grant(http, &form)
            .await
            .map_err(provider_failed)?;
        let account = site(entry)
            .account(http, &grant.access_token)
            .await
            .map_err(provider_failed)?;
        Ok((grant, account))
    }

    /// The access token of a connected connection, renewed when it expires soon.
    pub async fn git_access_token(
        &self,
        id: ShortId,
    ) -> anyhow::Result<(GitConnectionEntry, String)> {
        let entry = self.git_connection(id).await?;
        if entry.status != ConnectionStatus::Connected {
            return Err(Error::GitConnectionNotConnected { id }.into());
        }
        let stored = self.connection_secrets(id).await?;
        let token = if needs_renewal(stored.expires_at, unix_ms()) {
            self.renew_git_token(&entry).await?
        } else {
            let sealed = stored.access_token.context("the connection has no token")?;
            self.open_connection_value(id, "access_token", &sealed)?
        };
        Ok((entry, token))
    }

    /// Renews the access token. One renewal runs at a time, so a second caller finds the token
    /// that the first stored.
    async fn renew_git_token(&self, entry: &GitConnectionEntry) -> anyhow::Result<String> {
        let _renewal = self.renewing.lock().await;
        let id = entry.id;
        let stored = self.connection_secrets(id).await?;
        if !needs_renewal(stored.expires_at, unix_ms())
            && let Some(sealed) = &stored.access_token
        {
            return self.open_connection_value(id, "access_token", sealed);
        }
        let Some(sealed_refresh) = stored.refresh_token else {
            return self
                .expire(entry, "the connection has no refresh token")
                .await;
        };
        let refresh = self.open_connection_value(id, "refresh_token", &sealed_refresh)?;
        let secret = self.client_secret(id).await?;
        let redirect_uri = stored.redirect_uri.unwrap_or_default();
        let form = [
            ("client_id", entry.client_id.as_str()),
            ("client_secret", secret.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
        ];
        match site(entry).grant(self.http().await?, &form).await {
            Ok(grant) => {
                let tokens = self.seal_grant(id, &grant, Some(sealed_refresh))?;
                self.store.renew_git_tokens(id, &tokens.stored()).await?;
                Ok(grant.access_token)
            }
            Err(GrantError::Refused(reason)) => self.expire(entry, &reason).await,
            Err(GrantError::Failed(err)) => Err(provider_failed(err)),
        }
    }

    /// Marks a connection expired after the provider refused its tokens.
    async fn expire(&self, entry: &GitConnectionEntry, reason: &str) -> anyhow::Result<String> {
        warn!(
            connection = %entry.id,
            reason,
            "the Git provider refused the token renewal, so the connection needs a new authorization"
        );
        self.store.expire_git_connection(entry.id).await?;
        Err(Error::GitConnectionNotConnected { id: entry.id }.into())
    }

    /// Seals the tokens of a grant. A renewal without a new refresh token keeps the old one.
    fn seal_grant(
        &self,
        id: ShortId,
        grant: &TokenGrant,
        old_refresh: Option<Vec<u8>>,
    ) -> anyhow::Result<SealedTokens> {
        let access_aad = connection_aad(id, "access_token");
        let refresh_aad = connection_aad(id, "refresh_token");
        let refresh = match &grant.refresh_token {
            Some(token) => Some(self.keys.seal(&refresh_aad, token.as_bytes())?),
            None => old_refresh,
        };
        Ok(SealedTokens {
            access: self.keys.seal(&access_aad, grant.access_token.as_bytes())?,
            refresh,
            expires_at: grant
                .expires_in
                .map(|seconds| unix_ms().saturating_add(seconds.saturating_mul(1000))),
        })
    }

    async fn connection_secrets(
        &self,
        id: ShortId,
    ) -> anyhow::Result<super::store::StoredConnection> {
        self.store
            .git_connection_secrets(id)
            .await?
            .ok_or_else(|| not_found(id))
    }

    async fn client_secret(&self, id: ShortId) -> anyhow::Result<String> {
        let stored = self.connection_secrets(id).await?;
        self.open_connection_value(id, "client_secret", &stored.client_secret)
    }

    fn open_connection_value(
        &self,
        id: ShortId,
        field: &str,
        sealed: &[u8],
    ) -> anyhow::Result<String> {
        let value = self.keys.open(&connection_aad(id, field), sealed)?;
        String::from_utf8(value).with_context(|| format!("the {field} is not UTF-8"))
    }
}

/// The provider of a connection at its address.
pub(super) fn site(entry: &GitConnectionEntry) -> Site<'_> {
    Site {
        provider: entry.provider,
        url: &entry.url,
    }
}

/// The sealed tokens of a grant.
struct SealedTokens {
    access: Vec<u8>,
    refresh: Option<Vec<u8>>,
    expires_at: Option<u64>,
}

impl SealedTokens {
    fn stored(&self) -> StoredTokens<'_> {
        StoredTokens {
            access_token: &self.access,
            refresh_token: self.refresh.as_deref(),
            expires_at: self.expires_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{TempDir, platform};
    use super::*;
    use mockito::{Matcher, Mock, ServerGuard};
    use r3v3rs3_api::git_connection::{GitConnectionRequest, GitProvider};
    use std::sync::Arc;

    const CALLBACK: &str = "https://r3v3rs3.example.com/oauth/git/callback";

    struct Setup {
        platform: Arc<Platform>,
        server: ServerGuard,
        id: ShortId,
        _dir: TempDir,
    }

    /// A platform with a Gitea connection on a mock server.
    async fn setup() -> anyhow::Result<Setup> {
        let (platform, dir) = platform().await?;
        let server = mockito::Server::new_async().await;
        let request = GitConnectionRequest {
            name: "team".parse()?,
            provider: GitProvider::Gitea,
            url: Some(server.url().parse()?),
            client_id: "client-id".into(),
            client_secret: Some("client-secret".into()),
        };
        let id = platform.add_git_connection(request, 1).await?.id;
        Ok(Setup {
            platform,
            server,
            id,
            _dir: dir,
        })
    }

    fn token_body(access: &str, refresh: Option<&str>, expires_in: u64) -> String {
        serde_json::json!({
            "access_token": access,
            "refresh_token": refresh,
            "token_type": "bearer",
            "expires_in": expires_in,
        })
        .to_string()
    }

    async fn token_mock(server: &mut ServerGuard, form: Vec<Matcher>, body: String) -> Mock {
        server
            .mock("POST", "/login/oauth/access_token")
            .match_body(Matcher::AllOf(form))
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await
    }

    fn field(name: &str, value: &str) -> Matcher {
        Matcher::UrlEncoded(name.into(), value.into())
    }

    /// Authorizes the connection with the code `code` and an access token that expires in
    /// `expires_in` seconds.
    async fn connect(setup: &mut Setup, expires_in: u64) -> anyhow::Result<Authorized> {
        let redirect: RedirectUri = CALLBACK.parse()?;
        let started = setup
            .platform
            .authorize_git_connection(setup.id, &redirect, "admin", None)
            .await?;
        let url = url::Url::parse(&started.url)?;
        let query = url.query_pairs().collect::<HashMap<_, _>>();
        assert_eq!(query["client_id"], "client-id");
        assert_eq!(query["redirect_uri"], CALLBACK);
        assert_eq!(query["code_challenge_method"], "S256");
        let state = query["state"].to_string();
        let form = vec![
            field("grant_type", "authorization_code"),
            field("code", "the-code"),
            field("client_secret", "client-secret"),
            field("redirect_uri", CALLBACK),
            Matcher::Regex("code_verifier=[A-Za-z0-9_-]{43}".into()),
        ];
        let body = token_body("access-1", Some("refresh-1"), expires_in);
        let token = token_mock(&mut setup.server, form, body).await;
        let user = setup
            .server
            .mock("GET", "/api/v1/user")
            .match_header("authorization", "Bearer access-1")
            .with_body(r#"{"login":"alice"}"#)
            .create_async()
            .await;
        let authorized = setup
            .platform
            .finish_git_authorization(&state, "the-code")
            .await?;
        token.assert_async().await;
        user.assert_async().await;
        let again = setup
            .platform
            .finish_git_authorization(&state, "the-code")
            .await;
        assert!(matches!(
            again.unwrap_err().downcast::<Error>()?,
            Error::OauthStateInvalid
        ));
        Ok(authorized)
    }

    #[tokio::test]
    async fn an_authorization_stores_the_tokens_once() -> anyhow::Result<()> {
        let mut setup = setup().await?;
        let authorized = connect(&mut setup, 3600).await?;
        assert_eq!(authorized.username, "admin");
        assert_eq!(authorized.entry.status, ConnectionStatus::Connected);
        assert_eq!(authorized.entry.account.as_deref(), Some("alice"));
        let (_, token) = setup.platform.git_access_token(setup.id).await?;
        assert_eq!(token, "access-1");
        let stored = setup.platform.connection_secrets(setup.id).await?;
        let sealed = stored.access_token.context("token")?;
        assert!(!sealed.windows(8).any(|w| w == b"access-1"));
        Ok(())
    }

    #[tokio::test]
    async fn an_unknown_state_is_refused() -> anyhow::Result<()> {
        let setup = setup().await?;
        let err = setup
            .platform
            .finish_git_authorization("forged", "code")
            .await
            .unwrap_err();
        assert!(matches!(err.downcast::<Error>()?, Error::OauthStateInvalid));
        Ok(())
    }

    #[tokio::test]
    async fn a_token_that_expires_soon_is_renewed_once() -> anyhow::Result<()> {
        let mut setup = setup().await?;
        connect(&mut setup, 30).await?;
        let form = vec![
            field("grant_type", "refresh_token"),
            field("refresh_token", "refresh-1"),
            field("redirect_uri", CALLBACK),
        ];
        let renewal = token_mock(&mut setup.server, form, token_body("access-2", None, 3600))
            .await
            .expect(1);
        let (first, second) = tokio::join!(
            setup.platform.git_access_token(setup.id),
            setup.platform.git_access_token(setup.id)
        );
        assert_eq!(first?.1, "access-2");
        assert_eq!(second?.1, "access-2");
        renewal.assert_async().await;
        // A renewal without a new refresh token keeps the old one.
        let stored = setup.platform.connection_secrets(setup.id).await?;
        let refresh = stored.refresh_token.context("refresh token")?;
        let opened = setup
            .platform
            .open_connection_value(setup.id, "refresh_token", &refresh)?;
        assert_eq!(opened, "refresh-1");
        Ok(())
    }

    #[tokio::test]
    async fn a_refused_renewal_expires_the_connection() -> anyhow::Result<()> {
        let mut setup = setup().await?;
        connect(&mut setup, 30).await?;
        setup
            .server
            .mock("POST", "/login/oauth/access_token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant"}"#)
            .create_async()
            .await;
        let err = setup.platform.git_access_token(setup.id).await.unwrap_err();
        assert!(matches!(
            err.downcast::<Error>()?,
            Error::GitConnectionNotConnected { .. }
        ));
        let entry = setup.platform.git_connection(setup.id).await?;
        assert_eq!(entry.status, ConnectionStatus::Expired);
        let stored = setup.platform.connection_secrets(setup.id).await?;
        assert!(stored.access_token.is_none() && stored.refresh_token.is_none());
        Ok(())
    }

    #[test]
    fn the_challenge_is_the_sha256_of_the_verifier() {
        // The example of RFC 7636, appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_of(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(random_word().len(), 43);
        assert!(needs_renewal(Some(1_000), 950));
        assert!(!needs_renewal(Some(100_000), 1_000));
        assert!(!needs_renewal(None, u64::MAX));
    }
}
