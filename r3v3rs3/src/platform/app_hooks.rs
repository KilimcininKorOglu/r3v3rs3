//! The webhooks that the Git provider connection of an app installs at its repository. A webhook
//! sends the push events to `POST /hooks/apps/{id}`, signed with the webhook secret of the app.
//! A failed installation does not fail the change of the app: the app shows the error, and an
//! account installs the webhook again.

use super::Platform;
use super::store::StoredHook;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::git_connection::RepoName;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, AppSpec};
use tracing::warn;

/// The connection and the repository that the webhook of an app belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HookTarget {
    pub connection: ShortId,
    pub repository: RepoName,
}

impl HookTarget {
    fn of(hook: &StoredHook) -> anyhow::Result<Self> {
        Ok(Self {
            connection: hook.connection,
            repository: hook.repository.parse()?,
        })
    }

    /// Whether the stored webhook is installed for this connection and repository.
    fn holds(&self, hook: &StoredHook) -> bool {
        hook.remote_id.is_some()
            && hook.connection == self.connection
            && hook.repository == self.repository.as_str()
    }
}

impl Platform {
    /// The webhook target of a spec whose source has a connection. The connection must exist,
    /// and the repository must be one of its provider.
    pub(super) async fn hook_target(&self, spec: &AppSpec) -> anyhow::Result<Option<HookTarget>> {
        let Some((repository, Some(connection))) = spec.source.repository() else {
            return Ok(None);
        };
        let entry = self.git_connection(connection).await?;
        let repository =
            RepoName::of_clone_url(&entry.url, repository.as_str()).ok_or_else(|| {
                Error::InvalidGitConnection {
                    reason: format!("the repository {repository} is not at {}", entry.url),
                }
            })?;
        Ok(Some(HookTarget {
            connection,
            repository,
        }))
    }

    /// Brings the webhook of an app in line with its source after the app was stored: removes
    /// the webhook of an old repository and installs the webhook of the new one.
    pub(super) async fn sync_app_hook(
        &self,
        app: ShortId,
        target: Option<HookTarget>,
        now: u64,
    ) -> anyhow::Result<()> {
        let current = self.store.app_hook(app).await?;
        if let (Some(hook), Some(target)) = (&current, &target)
            && target.holds(hook)
        {
            return Ok(());
        }
        if let Some(old) = &current {
            self.remove_remote_hook(old).await;
            self.store.delete_app_hook(app).await?;
        }
        match target {
            Some(target) => self.install_app_hook(app, &target, now).await,
            None => Ok(()),
        }
    }

    /// Installs the webhook of an app again, and returns the app with the result.
    pub async fn reinstall_app_hook(&self, id: ShortId, now: u64) -> anyhow::Result<AppEntry> {
        let app = self.app(id).await?;
        let target =
            self.hook_target(&app.spec)
                .await?
                .ok_or_else(|| Error::InvalidGitConnection {
                    reason: "the app has no Git provider connection".to_string(),
                })?;
        if self.platform_settings().await?.public_url.is_none() {
            return Err(Error::PublicUrlMissing.into());
        }
        let old = self.store.app_hook(id).await?;
        self.replace_hook(id, old.as_ref(), &target, now).await?;
        self.app(id).await
    }

    /// Installs a stored webhook again, for example after the webhook secret changed.
    pub(super) async fn reinstall_stored_hook(
        &self,
        app: ShortId,
        hook: &StoredHook,
        now: u64,
    ) -> anyhow::Result<()> {
        let target = HookTarget::of(hook)?;
        self.replace_hook(app, Some(hook), &target, now).await
    }

    /// Removes the webhook of a deleted app at its provider.
    pub(super) async fn drop_app_hook(&self, app: ShortId) -> anyhow::Result<()> {
        if let Some(hook) = self.store.app_hook(app).await? {
            self.remove_remote_hook(&hook).await;
        }
        Ok(())
    }

    async fn replace_hook(
        &self,
        app: ShortId,
        old: Option<&StoredHook>,
        target: &HookTarget,
        now: u64,
    ) -> anyhow::Result<()> {
        if let Some(old) = old {
            self.remove_remote_hook(old).await;
        }
        self.install_app_hook(app, target, now).await
    }

    /// Installs the webhook and stores its id, or the reason of the failure.
    async fn install_app_hook(
        &self,
        app: ShortId,
        target: &HookTarget,
        now: u64,
    ) -> anyhow::Result<()> {
        let (remote_id, error, failure) = match self.create_app_hook(app, target).await {
            Ok(remote_id) => (Some(remote_id), None, None),
            Err(err) => {
                warn!(app = %app, "failed to install the webhook of the app: {err:#}");
                let failure = err
                    .downcast_ref::<Error>()
                    .map(serde_json::to_string)
                    .transpose()?;
                (None, Some(format!("{err:#}")), failure)
            }
        };
        let hook = StoredHook {
            connection: target.connection,
            repository: target.repository.to_string(),
            remote_id,
            error,
            failure,
            updated_at: now,
        };
        self.store.set_app_hook(app, &hook).await
    }

    async fn create_app_hook(&self, app: ShortId, target: &HookTarget) -> anyhow::Result<String> {
        let public_url = self
            .platform_settings()
            .await?
            .public_url
            .ok_or(Error::PublicUrlMissing)?;
        let secret = self.ensure_webhook_secret(app).await?;
        let url = format!("{public_url}/hooks/apps/{app}");
        self.create_git_hook(target.connection, &target.repository, &url, &secret)
            .await
    }

    /// Removes a webhook at its provider. A failure only logs a warning, because the provider
    /// then gets 404 for the requests of the webhook.
    async fn remove_remote_hook(&self, hook: &StoredHook) {
        let Some(remote_id) = &hook.remote_id else {
            return;
        };
        let result = match HookTarget::of(hook) {
            Ok(target) => {
                self.delete_git_hook(target.connection, &target.repository, remote_id)
                    .await
            }
            Err(err) => Err(err),
        };
        if let Err(err) = result {
            warn!(
                connection = %hook.connection,
                repository = hook.repository,
                "failed to remove an old webhook: {err:#}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::connections::connection_aad;
    use super::super::store::StoredTokens;
    use super::super::tests::{TempDir, api_error, platform, request};
    use super::*;
    use anyhow::Context as _;
    use mockito::{Matcher, ServerGuard};
    use r3v3rs3_api::git_connection::{GitConnectionRequest, GitProvider, PlatformSettings};
    use r3v3rs3_api::platform::AppHook;
    use r3v3rs3_api::platform::{AppRequest, AppSource};
    use std::sync::Arc;

    /// A Gitea connection at `url`.
    async fn add_connection(platform: &Platform, url: &str) -> anyhow::Result<ShortId> {
        let request = GitConnectionRequest {
            name: "team".parse()?,
            provider: GitProvider::Gitea,
            url: Some(url.parse()?),
            client_id: "client-id".into(),
            client_secret: Some("client-secret".into()),
        };
        Ok(platform.add_git_connection(request, 1).await?.id)
    }

    /// Stores the access token `token` of the connection, as an authorization does.
    async fn connect(platform: &Platform, id: ShortId, token: &str) -> anyhow::Result<()> {
        let sealed = platform
            .keys
            .seal(&connection_aad(id, "access_token"), token.as_bytes())?;
        let tokens = StoredTokens {
            access_token: &sealed,
            refresh_token: None,
            expires_at: None,
        };
        platform
            .store
            .connect_git_connection(id, "alice", "https://r.example/cb", &tokens)
            .await?;
        Ok(())
    }

    fn git_request(repository: &str, connection: ShortId) -> anyhow::Result<AppRequest> {
        let mut site = request("site");
        site.spec.source = AppSource::Git {
            repository: repository.parse()?,
            branch: "main".parse()?,
            context: ".".parse()?,
            dockerfile: "Dockerfile".parse()?,
            connection: Some(connection),
        };
        Ok(site)
    }

    /// A platform with a public address, an app `site` and a Gitea connection with a token on
    /// a mock server.
    struct Mocked {
        platform: Arc<Platform>,
        server: ServerGuard,
        connection: ShortId,
        app: ShortId,
        _dir: TempDir,
    }

    impl Mocked {
        async fn new() -> anyhow::Result<Self> {
            let (platform, dir) = platform().await?;
            let server = mockito::Server::new_async().await;
            let connection = add_connection(&platform, &server.url()).await?;
            connect(&platform, connection, "access-1").await?;
            let settings = PlatformSettings {
                public_url: Some("https://deploy.example.com".parse()?),
            };
            platform.set_platform_settings(&settings).await?;
            let app = platform.add_app(request("site"), 1).await?.id;
            Ok(Self {
                platform,
                server,
                connection,
                app,
                _dir: dir,
            })
        }

        fn target(&self) -> HookTarget {
            HookTarget {
                connection: self.connection,
                repository: "team/site".parse().unwrap(),
            }
        }

        async fn sync(&self, target: Option<HookTarget>, now: u64) -> anyhow::Result<()> {
            self.platform.sync_app_hook(self.app, target, now).await
        }

        async fn hook(&self) -> anyhow::Result<Option<AppHook>> {
            Ok(self.platform.app(self.app).await?.hook)
        }
    }

    #[tokio::test]
    async fn an_app_needs_a_repository_of_its_connection() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let connection = add_connection(&platform, "https://git.example.com").await?;
        let elsewhere = git_request("https://github.com/team/site.git", connection)?;
        let err = platform.add_app(elsewhere, 1).await.unwrap_err();
        assert!(matches!(api_error(err), Error::InvalidGitConnection { .. }));
        let unknown = git_request("https://git.example.com/team/site.git", "zzz".parse()?)?;
        let err = platform.add_app(unknown, 1).await.unwrap_err();
        assert!(matches!(api_error(err), Error::IdNotFound { .. }));
        Ok(())
    }

    #[tokio::test]
    async fn an_app_keeps_the_reason_of_its_missing_webhook() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let connection = add_connection(&platform, "https://git.example.com").await?;
        let site = git_request("https://git.example.com/team/site.git", connection)?;
        let app = platform.add_app(site, 2).await?;
        let hook = app.hook.context("hook")?;
        assert_eq!(
            (hook.repository.as_str(), hook.installed),
            ("team/site", false)
        );
        assert!(hook.error.unwrap_or_default().contains("public address"));
        // The client translates the API error.
        assert!(
            hook.failure
                .unwrap_or_default()
                .contains("public_url_missing")
        );
        let err = platform.reinstall_app_hook(app.id, 3).await.unwrap_err();
        assert!(matches!(api_error(err), Error::PublicUrlMissing));

        let err = platform
            .delete_git_connection(connection)
            .await
            .unwrap_err();
        assert!(matches!(api_error(err), Error::GitConnectionInUse { name } if name == "site"));
        platform.delete_app(app.id).await?;
        platform.delete_git_connection(connection).await?;
        Ok(())
    }

    #[tokio::test]
    async fn a_connection_installs_and_keeps_the_webhook() -> anyhow::Result<()> {
        let mut mocked = Mocked::new().await?;
        let hook_url = format!("https://deploy.example.com/hooks/apps/{}", mocked.app);
        let create = mocked
            .server
            .mock("POST", "/api/v1/repos/team/site/hooks")
            .match_header("authorization", "Bearer access-1")
            .match_body(Matcher::PartialJson(
                serde_json::json!({"config": {"url": hook_url}}),
            ))
            .with_status(201)
            .with_body(r#"{"id":7}"#)
            .expect(1)
            .create_async()
            .await;
        mocked.sync(Some(mocked.target()), 2).await?;
        // The same repository keeps its webhook.
        mocked.sync(Some(mocked.target()), 3).await?;
        create.assert_async().await;
        let app = mocked.platform.app(mocked.app).await?;
        assert!(app.webhook_secret_set);
        let hook = app.hook.context("hook")?;
        assert_eq!(
            (hook.installed, hook.error, hook.updated_at),
            (true, None, 2)
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_app_without_the_connection_loses_its_webhook() -> anyhow::Result<()> {
        let mut mocked = Mocked::new().await?;
        mocked
            .server
            .mock("POST", "/api/v1/repos/team/site/hooks")
            .with_status(201)
            .with_body(r#"{"id":7}"#)
            .create_async()
            .await;
        let delete = mocked
            .server
            .mock("DELETE", "/api/v1/repos/team/site/hooks/7")
            .with_status(204)
            .expect(1)
            .create_async()
            .await;
        mocked.sync(Some(mocked.target()), 2).await?;
        mocked.sync(None, 3).await?;
        delete.assert_async().await;
        assert_eq!(mocked.hook().await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn a_new_secret_installs_the_webhook_again() -> anyhow::Result<()> {
        let mut mocked = Mocked::new().await?;
        let create = mocked
            .server
            .mock("POST", "/api/v1/repos/team/site/hooks")
            .with_status(201)
            .with_body(r#"{"id":7}"#)
            .expect(2)
            .create_async()
            .await;
        let delete = mocked
            .server
            .mock("DELETE", "/api/v1/repos/team/site/hooks/7")
            .with_status(204)
            .expect(2)
            .create_async()
            .await;
        mocked.sync(Some(mocked.target()), 2).await?;
        mocked.platform.new_webhook_secret(mocked.app).await?;
        let app = mocked.platform.delete_webhook_secret(mocked.app).await?;
        create.assert_async().await;
        delete.assert_async().await;
        assert_eq!((app.webhook_secret_set, app.hook), (false, None));
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_installation_keeps_its_reason() -> anyhow::Result<()> {
        let mut mocked = Mocked::new().await?;
        mocked
            .server
            .mock("POST", "/api/v1/repos/team/site/hooks")
            .with_status(403)
            .with_body(r#"{"message":"no admin rights"}"#)
            .create_async()
            .await;
        mocked.sync(Some(mocked.target()), 2).await?;
        let hook = mocked.hook().await?.context("hook")?;
        assert!(!hook.installed);
        let error = hook.error.unwrap_or_default();
        assert!(error.contains("403"), "{error}");
        Ok(())
    }

    #[tokio::test]
    async fn a_connection_clones_with_its_token() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let connection = add_connection(&platform, "https://git.example.com").await?;
        let app: ShortId = "bcd-fgh".parse()?;
        let err = platform.git_credential(app, Some(connection)).await;
        assert!(matches!(
            api_error(err.unwrap_err()),
            Error::GitConnectionNotConnected { .. }
        ));
        connect(&platform, connection, "access-1").await?;
        let credential = platform.git_credential(app, Some(connection)).await?;
        let credential = credential.context("credential")?;
        assert_eq!(
            (credential.user.as_str(), credential.password.as_str()),
            ("access-1", "x-oauth-basic")
        );
        Ok(())
    }
}
