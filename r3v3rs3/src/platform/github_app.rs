//! The GitHub App of a GitHub connection. An admin posts a manifest to GitHub, GitHub creates the
//! GitHub App and sends the browser back to the public callback with a code, and the platform trades
//! the code for the client values and the private key of the GitHub App. After the admin installs
//! the GitHub App, a JWT signed with its private key asks for the tokens of the installation.

use super::connections::connection_aad;
use super::oauth::{Authorized, Pending, Purpose, provider_failed, random_word, site};
use super::provider::{Conversion, Site, TokenError};
use super::store::{StoredGithubApp, StoredTokens, Write};
use super::{Platform, not_found};
use crate::clock::unix_ms;
use anyhow::{Context as _, anyhow, bail};
use r3v3rs3_api::container::AppName;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::git_connection::{
    AuthorizeResponse, ConnectionStatus, GitConnectionEntry, GitProvider, GithubAppForm,
    GithubAppRequest, OAUTH_CALLBACK_PATH, ProviderUrl, RedirectUri,
};
use r3v3rs3_api::id::ShortId;
use ring::signature::RsaKeyPair;
use serde_json::{Value, json};
use std::net::IpAddr;
use std::time::Instant;
use tokio_rustls::rustls::pki_types::PrivateKeyDer;

/// The longest name of a GitHub App.
const MAX_APP_NAME_LENGTH: usize = 34;

/// The lifetime of the JWT of a GitHub App in seconds. GitHub accepts at most ten minutes.
const JWT_LIFETIME_SECS: u64 = 540;

/// The seconds that the JWT dates back, against a clock that runs behind the one of GitHub.
const JWT_CLOCK_DRIFT_SECS: u64 = 60;

/// The WebUI address of a callback address.
fn origin(redirect_uri: &RedirectUri) -> &str {
    let uri = redirect_uri.as_str();
    uri.strip_suffix(OAUTH_CALLBACK_PATH).unwrap_or(uri)
}

/// The manifest of the GitHub App of connection `id`. The GitHub App reads the contents of the
/// repositories that an admin installs it on and manages their webhooks. It has no webhook of its
/// own, because each app gets a webhook on its repository.
fn manifest(name: &AppName, id: ShortId, redirect_uri: &RedirectUri) -> Value {
    let origin = origin(redirect_uri);
    let app_name = format!("r3v3rs3-{name}")
        .chars()
        .take(MAX_APP_NAME_LENGTH)
        .collect::<String>();
    json!({
        "name": app_name,
        "url": origin,
        "redirect_url": redirect_uri.as_str(),
        "setup_url": format!("{origin}/git_connections?installed={id}"),
        "setup_on_update": true,
        "public": false,
        "default_permissions": {
            "contents": "read",
            "metadata": "read",
            "repository_hooks": "write",
        },
    })
}

/// Checks the page of a new GitHub App: it lies on the GitHub server of the connection. The
/// callback writes the installation page into an HTML attribute, so the address holds no quote,
/// angle bracket, backslash, space or control character.
fn check_html_url(url: &ProviderUrl, html_url: &str) -> anyhow::Result<()> {
    let valid = html_url.starts_with(&format!("{url}/"))
        && url::Url::parse(html_url).is_ok()
        && !html_url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "\"'<>\\`".contains(c));
    if valid {
        Ok(())
    } else {
        bail!("GitHub returned an unexpected address for the new GitHub App")
    }
}

/// The installation page of a GitHub App.
fn install_url(html_url: &str) -> String {
    format!("{html_url}/installations/new")
}

/// A JWT of the GitHub App with the client id `client_id`, signed with its private key `pem`.
fn app_jwt(client_id: &str, pem: &str, now_secs: u64) -> anyhow::Result<String> {
    let key = rustls_pemfile::private_key(&mut pem.as_bytes())
        .ok()
        .flatten()
        .context("the private key of the GitHub App is not PEM")?;
    let pair = match &key {
        PrivateKeyDer::Pkcs1(key) => RsaKeyPair::from_der(key.secret_pkcs1_der()),
        PrivateKeyDer::Pkcs8(key) => RsaKeyPair::from_pkcs8(key.secret_pkcs8_der()),
        _ => bail!("the private key of the GitHub App is not an RSA key"),
    }
    .map_err(|_| anyhow!("the private key of the GitHub App is not a valid RSA key"))?;
    let claims = json!({
        "iat": now_secs.saturating_sub(JWT_CLOCK_DRIFT_SECS),
        "exp": now_secs.saturating_add(JWT_LIFETIME_SECS),
        "iss": client_id,
    });
    crate::jwt::rs256(&pair, &claims)
}

impl Platform {
    /// Starts the creation of a GitHub App and returns the form that the browser of the admin
    /// posts to GitHub.
    pub async fn create_github_app(
        &self,
        request: &GithubAppRequest,
        username: &str,
        client: Option<IpAddr>,
    ) -> anyhow::Result<GithubAppForm> {
        let url = request.provider_url()?;
        let taken = self.store.git_connections().await?;
        if taken.iter().any(|entry| entry.name == request.name) {
            return Err(Error::GitConnectionNameExists {
                name: request.name.to_string(),
            }
            .into());
        }
        let id = self.free_connection_id().await?;
        let state = random_word();
        let github = Site {
            provider: GitProvider::Github,
            url: &url,
        };
        let form = GithubAppForm {
            url: github.manifest_form_url(request.organization.as_ref(), &state)?,
            manifest: manifest(&request.name, id, &request.redirect_uri).to_string(),
        };
        let pending = Pending {
            connection: id,
            redirect_uri: request.redirect_uri.clone(),
            purpose: Purpose::CreateApp {
                name: request.name.clone(),
                url,
            },
            username: username.to_string(),
            client,
            created: Instant::now(),
        };
        self.pending.insert(state, pending);
        Ok(form)
    }

    /// Trades the code of the manifest callback for the GitHub App and stores its connection. The
    /// admin installs the GitHub App next, on the returned page.
    pub(super) async fn finish_github_app(
        &self,
        pending: Pending,
        name: AppName,
        url: ProviderUrl,
        code: &str,
    ) -> anyhow::Result<Authorized> {
        let github = Site {
            provider: GitProvider::Github,
            url: &url,
        };
        let app = github
            .convert_manifest(self.http().await?, code)
            .await
            .map_err(provider_failed)?;
        check_html_url(&url, &app.html_url).map_err(provider_failed)?;
        let install = install_url(&app.html_url);
        let id = pending.connection;
        self.store_github_app(id, name, url, app).await?;
        Ok(Authorized {
            entry: self.git_connection(id).await?,
            username: pending.username,
            client: pending.client,
            install_url: Some(install),
        })
    }

    /// Stores the connection of a new GitHub App with its sealed client secret and private key.
    async fn store_github_app(
        &self,
        id: ShortId,
        name: AppName,
        url: ProviderUrl,
        app: Conversion,
    ) -> anyhow::Result<()> {
        let now = unix_ms();
        let entry = GitConnectionEntry {
            id,
            name,
            provider: GitProvider::Github,
            url,
            client_id: app.client_id,
            status: ConnectionStatus::NotConnected,
            account: None,
            app_url: Some(app.html_url.clone()),
            created_at: now,
            updated_at: now,
        };
        let secret = self.keys.seal(
            &connection_aad(id, "client_secret"),
            app.client_secret.as_bytes(),
        )?;
        let stored = StoredGithubApp {
            html_url: app.html_url,
            private_key: self
                .keys
                .seal(&connection_aad(id, "private_key"), app.pem.as_bytes())?,
            installation_id: None,
        };
        match self
            .store
            .insert_github_app(&entry, &secret, &stored)
            .await?
        {
            Write::Done => Ok(()),
            Write::NameTaken => Err(Error::GitConnectionNameExists {
                name: entry.name.to_string(),
            }
            .into()),
            Write::NotFound => Err(anyhow!("the connection was not stored")),
        }
    }

    /// Finds the installation of the GitHub App of a connection and connects the connection. The
    /// `installation_id` that GitHub adds to the setup address is not trusted: the list of the
    /// installations comes from GitHub itself.
    pub async fn install_github_app(&self, id: ShortId) -> anyhow::Result<GitConnectionEntry> {
        let (entry, app) = self.github_app(id).await?;
        let jwt = self.github_jwt(&entry, &app)?;
        let installation = site(&entry)
            .installation(self.http().await?, &jwt)
            .await
            .map_err(provider_failed)?
            .ok_or(Error::GithubAppNotInstalled)?;
        if !self
            .store
            .install_github_app(id, installation.id, &installation.account)
            .await?
        {
            return Err(not_found(id));
        }
        self.git_connection(id).await
    }

    /// The installation page of the GitHub App of a connection.
    pub(super) async fn github_install_page(
        &self,
        id: ShortId,
    ) -> anyhow::Result<AuthorizeResponse> {
        let (_, app) = self.github_app(id).await?;
        Ok(AuthorizeResponse {
            url: install_url(&app.html_url),
        })
    }

    /// A new token of the installation. The caller holds the renewal lock.
    pub(super) async fn renew_installation_token(
        &self,
        entry: &GitConnectionEntry,
    ) -> anyhow::Result<String> {
        let (_, app) = self.github_app(entry.id).await?;
        let Some(installation) = app.installation_id else {
            return Err(Error::GitConnectionNotConnected { id: entry.id }.into());
        };
        let jwt = self.github_jwt(entry, &app)?;
        let now = unix_ms();
        match site(entry)
            .installation_token(self.http().await?, &jwt, installation, now)
            .await
        {
            Ok(granted) => {
                let aad = connection_aad(entry.id, "access_token");
                let sealed = self.keys.seal(&aad, granted.token.as_bytes())?;
                let tokens = StoredTokens {
                    access_token: &sealed,
                    refresh_token: None,
                    expires_at: Some(granted.expires_at),
                };
                self.store.renew_git_tokens(entry.id, &tokens).await?;
                Ok(granted.token)
            }
            Err(TokenError::Gone) => {
                self.expire(entry, "the GitHub App is not installed anymore")
                    .await
            }
            Err(TokenError::Failed(err)) => Err(provider_failed(err)),
        }
    }

    /// A GitHub connection with its GitHub App.
    async fn github_app(
        &self,
        id: ShortId,
    ) -> anyhow::Result<(GitConnectionEntry, StoredGithubApp)> {
        let entry = self.git_connection(id).await?;
        let app = self
            .store
            .github_app(id)
            .await?
            .ok_or(Error::GitConnectionNotConnected { id })?;
        Ok((entry, app))
    }

    fn github_jwt(
        &self,
        entry: &GitConnectionEntry,
        app: &StoredGithubApp,
    ) -> anyhow::Result<String> {
        let aad = connection_aad(entry.id, "private_key");
        let pem = String::from_utf8(self.keys.open(&aad, &app.private_key)?)
            .context("the private key of the GitHub App is not UTF-8")?;
        app_jwt(&entry.client_id, &pem, unix_ms() / 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{TempDir, platform};
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use mockito::ServerGuard;
    use ring::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use std::sync::Arc;

    const CALLBACK: &str = "https://r3v3rs3.example.com/oauth/git/callback";

    /// A new RSA private key in PKCS#1 PEM, as GitHub returns it.
    fn private_key() -> anyhow::Result<String> {
        let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)?;
        Ok(key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)?.to_string())
    }

    fn decoded(part: &str) -> anyhow::Result<Value> {
        Ok(serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part)?)?)
    }

    #[test]
    fn the_jwt_is_signed_with_the_private_key() -> anyhow::Result<()> {
        let pem = private_key()?;
        let jwt = app_jwt("Iv1.abc", &pem, 1_000_000)?;
        let parts = jwt.split('.').collect::<Vec<_>>();
        let [header, claims, signature] = parts.as_slice() else {
            bail!("the JWT has no three parts: {jwt}");
        };
        assert_eq!(decoded(header)?, json!({"alg": "RS256", "typ": "JWT"}));
        assert_eq!(
            decoded(claims)?,
            json!({"iat": 999_940, "exp": 1_000_540, "iss": "Iv1.abc"})
        );
        let der = rustls_pemfile::private_key(&mut pem.as_bytes())?.context("key")?;
        let PrivateKeyDer::Pkcs1(der) = der else {
            bail!("not PKCS#1");
        };
        let pair = RsaKeyPair::from_der(der.secret_pkcs1_der()).map_err(|err| anyhow!("{err}"))?;
        let public = UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, pair.public().as_ref());
        public
            .verify(
                format!("{header}.{claims}").as_bytes(),
                &URL_SAFE_NO_PAD.decode(signature)?,
            )
            .map_err(|_| anyhow!("the signature does not verify"))?;
        assert!(app_jwt("Iv1.abc", "not a key", 0).is_err());
        Ok(())
    }

    #[test]
    fn the_github_app_page_lies_on_its_server() -> anyhow::Result<()> {
        let url: ProviderUrl = "https://github.com".parse()?;
        assert!(check_html_url(&url, "https://github.com/apps/r3v3rs3-team").is_ok());
        for invalid in [
            "https://evil.example/apps/x",
            "https://github.com.evil.example/apps/x",
            "https://github.com/apps/x\"><script>",
            "https://github.com/apps/a b",
        ] {
            assert!(check_html_url(&url, invalid).is_err(), "{invalid}");
        }
        Ok(())
    }

    #[test]
    fn the_manifest_describes_the_github_app() -> anyhow::Result<()> {
        let redirect: RedirectUri = CALLBACK.parse()?;
        let name: AppName = "a-very-long-connection-name-for-github".parse()?;
        let id: ShortId = "bcd-fgh".parse()?;
        let manifest = manifest(&name, id, &redirect);
        let app_name = manifest["name"].as_str().context("name")?;
        assert_eq!(app_name.len(), MAX_APP_NAME_LENGTH);
        assert!(app_name.starts_with("r3v3rs3-a-very-long"));
        assert_eq!(manifest["url"], "https://r3v3rs3.example.com");
        assert_eq!(manifest["redirect_url"], CALLBACK);
        assert_eq!(
            manifest["setup_url"],
            "https://r3v3rs3.example.com/git_connections?installed=bcd-fgh"
        );
        assert_eq!(manifest["public"], false);
        assert_eq!(manifest["default_permissions"]["repository_hooks"], "write");
        assert!(manifest.get("hook_attributes").is_none());
        Ok(())
    }

    struct Setup {
        platform: Arc<Platform>,
        server: ServerGuard,
        _dir: TempDir,
    }

    async fn setup() -> anyhow::Result<Setup> {
        let (platform, dir) = platform().await?;
        Ok(Setup {
            platform,
            server: mockito::Server::new_async().await,
            _dir: dir,
        })
    }

    fn request(setup: &Setup, name: &str) -> anyhow::Result<GithubAppRequest> {
        Ok(GithubAppRequest {
            name: name.parse()?,
            url: Some(setup.server.url().parse()?),
            organization: Some("my-team".parse()?),
            redirect_uri: CALLBACK.parse()?,
        })
    }

    /// Creates the GitHub App of the connection `team` and returns it with the private key.
    async fn create(setup: &mut Setup) -> anyhow::Result<(Authorized, String)> {
        let started = setup
            .platform
            .create_github_app(&request(setup, "team")?, "admin", None)
            .await?;
        let url = url::Url::parse(&started.url)?;
        assert_eq!(url.path(), "/organizations/my-team/settings/apps/new");
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.to_string())
            .context("state")?;
        let pem = private_key()?;
        let body = json!({
            "html_url": format!("{}/apps/r3v3rs3-team", setup.server.url()),
            "client_id": "Iv1.abc",
            "client_secret": "client-s3cr3t",
            "webhook_secret": null,
            "pem": pem,
        });
        let conversion = setup
            .server
            .mock("POST", "/api/v3/app-manifests/the-code/conversions")
            .with_status(201)
            .with_body(body.to_string())
            .create_async()
            .await;
        let created = setup
            .platform
            .finish_git_authorization(&state, "the-code")
            .await?;
        conversion.assert_async().await;
        let again = setup
            .platform
            .finish_git_authorization(&state, "the-code")
            .await;
        assert!(matches!(
            again.unwrap_err().downcast::<Error>()?,
            Error::OauthStateInvalid
        ));
        Ok((created, pem))
    }

    #[tokio::test]
    async fn a_created_github_app_keeps_its_key_sealed() -> anyhow::Result<()> {
        let mut setup = setup().await?;
        let (created, pem) = create(&mut setup).await?;
        assert_eq!(created.username, "admin");
        let install_page = format!("{}/apps/r3v3rs3-team/installations/new", setup.server.url());
        assert_eq!(created.install_url.as_deref(), Some(install_page.as_str()));
        let entry = created.entry;
        assert_eq!(entry.provider, GitProvider::Github);
        assert_eq!(entry.status, ConnectionStatus::NotConnected);
        assert_eq!(entry.client_id, "Iv1.abc");
        let (_, app) = setup.platform.github_app(entry.id).await?;
        let marker = &pem.as_bytes()[40..80];
        assert!(!app.private_key.windows(marker.len()).any(|w| w == marker));
        let stored = setup.platform.connection_secrets(entry.id).await?;
        assert!(
            !stored
                .client_secret
                .windows(13)
                .any(|w| w == b"client-s3cr3t")
        );

        let taken = setup
            .platform
            .create_github_app(&request(&setup, "team")?, "admin", None)
            .await;
        assert!(matches!(
            taken.unwrap_err().downcast::<Error>()?,
            Error::GitConnectionNameExists { .. }
        ));
        let page = setup
            .platform
            .authorize_git_connection(entry.id, &CALLBACK.parse()?, "admin", None)
            .await?;
        assert_eq!(page.url, install_page);
        Ok(())
    }

    #[tokio::test]
    async fn a_github_app_connection_changes_only_its_name() -> anyhow::Result<()> {
        let mut setup = setup().await?;
        let (created, _) = create(&mut setup).await?;
        let entry = created.entry;
        let mut rename = r3v3rs3_api::git_connection::GitConnectionRequest {
            name: "renamed".parse()?,
            provider: GitProvider::Github,
            url: None,
            client_id: "Iv1.abc".into(),
            client_secret: None,
        };
        let renamed = setup
            .platform
            .update_git_connection(entry.id, rename.clone(), 2)
            .await?;
        assert_eq!(renamed.name.as_str(), "renamed");
        assert_eq!(renamed.app_url, entry.app_url);
        rename.client_secret = Some("other".into());
        let changed = setup
            .platform
            .update_git_connection(entry.id, rename, 3)
            .await;
        assert!(matches!(
            changed.unwrap_err().downcast::<Error>()?,
            Error::InvalidGitConnection { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn an_installation_connects_and_gives_tokens() -> anyhow::Result<()> {
        let mut setup = setup().await?;
        let (created, _) = create(&mut setup).await?;
        let id = created.entry.id;
        let none = setup
            .server
            .mock("GET", "/api/v3/app/installations")
            .with_body("[]")
            .create_async()
            .await;
        let missing = setup.platform.install_github_app(id).await.unwrap_err();
        assert!(matches!(
            missing.downcast::<Error>()?,
            Error::GithubAppNotInstalled
        ));
        none.remove_async().await;
        setup
            .server
            .mock("GET", "/api/v3/app/installations")
            .match_header(
                "authorization",
                mockito::Matcher::Regex("^Bearer [^.]+\\.[^.]+\\.[^.]+$".into()),
            )
            .with_body(r#"[{"id":77,"account":{"login":"my-team"}}]"#)
            .create_async()
            .await;
        let entry = setup.platform.install_github_app(id).await?;
        assert_eq!(entry.status, ConnectionStatus::Connected);
        assert_eq!(entry.account.as_deref(), Some("my-team"));

        let token = setup
            .server
            .mock("POST", "/api/v3/app/installations/77/access_tokens")
            .with_status(201)
            .with_body(r#"{"token":"ghs_one"}"#)
            .expect(1)
            .create_async()
            .await;
        let (first, second) = tokio::join!(
            setup.platform.git_access_token(id),
            setup.platform.git_access_token(id)
        );
        assert_eq!(first?.1, "ghs_one");
        assert_eq!(second?.1, "ghs_one");
        token.assert_async().await;
        let credential = setup.platform.connection_credential(id).await?;
        assert_eq!(
            (credential.user.as_str(), credential.password.as_str()),
            ("x-access-token", "ghs_one")
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_removed_installation_expires_the_connection() -> anyhow::Result<()> {
        let mut setup = setup().await?;
        let (created, _) = create(&mut setup).await?;
        let id = created.entry.id;
        setup
            .server
            .mock("GET", "/api/v3/app/installations")
            .with_body(r#"[{"id":77,"account":{"login":"my-team"}}]"#)
            .create_async()
            .await;
        setup.platform.install_github_app(id).await?;
        setup
            .server
            .mock("POST", "/api/v3/app/installations/77/access_tokens")
            .with_status(404)
            .create_async()
            .await;
        let err = setup.platform.git_access_token(id).await.unwrap_err();
        assert!(matches!(
            err.downcast::<Error>()?,
            Error::GitConnectionNotConnected { .. }
        ));
        let entry = setup.platform.git_connection(id).await?;
        assert_eq!(entry.status, ConnectionStatus::Expired);
        Ok(())
    }
}
