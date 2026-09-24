//! The Git provider connections and the settings of the platform. A connection keeps its client
//! secret and its tokens sealed with `platform.key`, and the admin API never returns them.

use super::store::{PUBLIC_URL, Write};
use super::{Platform, new_id, not_found};
use anyhow::anyhow;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::git_connection::{
    ConnectionStatus, GitConnectionEntry, GitConnectionRequest, PlatformSettings,
};
use r3v3rs3_api::id::ShortId;

/// The associated data of a sealed value binds it to its connection and its field.
pub(super) fn connection_aad(id: ShortId, field: &str) -> String {
    format!("git_connection/{id}/{field}")
}

impl Platform {
    pub async fn git_connections(&self) -> anyhow::Result<Vec<GitConnectionEntry>> {
        self.store.git_connections().await
    }

    pub async fn git_connection(&self, id: ShortId) -> anyhow::Result<GitConnectionEntry> {
        self.store
            .git_connection(id)
            .await?
            .ok_or_else(|| not_found(id))
    }

    pub async fn add_git_connection(
        &self,
        request: GitConnectionRequest,
        now: u64,
    ) -> anyhow::Result<GitConnectionEntry> {
        let url = request.validate()?;
        let secret =
            request
                .client_secret
                .as_deref()
                .ok_or_else(|| Error::InvalidGitConnection {
                    reason: "a new connection needs the client secret".to_string(),
                })?;
        let entry = GitConnectionEntry {
            id: self.free_connection_id().await?,
            name: request.name,
            provider: request.provider,
            url,
            client_id: request.client_id,
            status: ConnectionStatus::NotConnected,
            account: None,
            created_at: now,
            updated_at: now,
        };
        let aad = connection_aad(entry.id, "client_secret");
        let sealed = self.keys.seal(&aad, secret.as_bytes())?;
        match self.store.insert_git_connection(&entry, &sealed).await? {
            Write::Done => Ok(entry),
            Write::NameTaken => Err(connection_name_taken(&entry).into()),
            Write::NotFound => Err(anyhow!("the connection was not stored")),
        }
    }

    /// Replaces the settings of a connection. A new provider, address or client id disconnects
    /// it.
    pub async fn update_git_connection(
        &self,
        id: ShortId,
        request: GitConnectionRequest,
        now: u64,
    ) -> anyhow::Result<GitConnectionEntry> {
        let url = request.validate()?;
        let current = self.git_connection(id).await?;
        let entry = GitConnectionEntry {
            name: request.name,
            provider: request.provider,
            url,
            client_id: request.client_id,
            updated_at: now,
            ..current
        };
        let sealed = request
            .client_secret
            .map(|secret| {
                self.keys
                    .seal(&connection_aad(id, "client_secret"), secret.as_bytes())
            })
            .transpose()?;
        match self
            .store
            .update_git_connection(&entry, sealed.as_deref())
            .await?
        {
            Write::Done => self.git_connection(id).await,
            Write::NameTaken => Err(connection_name_taken(&entry).into()),
            Write::NotFound => Err(not_found(id)),
        }
    }

    pub async fn delete_git_connection(&self, id: ShortId) -> anyhow::Result<GitConnectionEntry> {
        let entry = self.git_connection(id).await?;
        if !self.store.delete_git_connection(id).await? {
            return Err(not_found(id));
        }
        Ok(entry)
    }

    pub async fn platform_settings(&self) -> anyhow::Result<PlatformSettings> {
        let public_url = self.store.setting(PUBLIC_URL).await?;
        Ok(PlatformSettings {
            public_url: public_url.map(|url| url.parse()).transpose()?,
        })
    }

    pub async fn set_platform_settings(&self, settings: &PlatformSettings) -> anyhow::Result<()> {
        let public_url = settings.public_url.as_ref().map(|url| url.as_str());
        self.store.set_setting(PUBLIC_URL, public_url).await
    }

    /// A new id that no connection uses.
    async fn free_connection_id(&self) -> anyhow::Result<ShortId> {
        for _ in 0..16 {
            let id = new_id()?;
            if self.store.git_connection(id).await?.is_none() {
                return Ok(id);
            }
        }
        Err(anyhow!("no free connection id"))
    }
}

fn connection_name_taken(entry: &GitConnectionEntry) -> Error {
    Error::GitConnectionNameExists {
        name: entry.name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::store::StoredTokens;
    use super::super::tests::platform;
    use super::*;
    use anyhow::Context as _;
    use r3v3rs3_api::git_connection::GitProvider;

    fn request(name: &str, secret: Option<&str>) -> GitConnectionRequest {
        GitConnectionRequest {
            name: name.parse().unwrap(),
            provider: GitProvider::Gitea,
            url: Some("https://git.example.com".parse().unwrap()),
            client_id: "client".into(),
            client_secret: secret.map(str::to_string),
        }
    }

    fn api_error(err: anyhow::Error) -> Error {
        err.downcast::<Error>().unwrap()
    }

    #[tokio::test]
    async fn a_connection_keeps_its_secret_sealed() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let err = platform.add_git_connection(request("team", None), 1).await;
        assert!(matches!(
            api_error(err.unwrap_err()),
            Error::InvalidGitConnection { .. }
        ));
        let entry = platform
            .add_git_connection(request("team", Some("s3cr3t")), 1)
            .await?;
        assert_eq!(entry.status, ConnectionStatus::NotConnected);
        let stored = platform
            .store
            .git_connection_secrets(entry.id)
            .await?
            .context("secrets")?;
        assert!(!stored.client_secret.windows(6).any(|w| w == b"s3cr3t"));
        let opened = platform.keys.open(
            &connection_aad(entry.id, "client_secret"),
            &stored.client_secret,
        )?;
        assert_eq!(opened, b"s3cr3t");

        let taken = platform
            .add_git_connection(request("team", Some("other")), 2)
            .await;
        assert!(matches!(
            api_error(taken.unwrap_err()),
            Error::GitConnectionNameExists { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn an_update_without_a_secret_keeps_it() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let entry = platform
            .add_git_connection(request("team", Some("s3cr3t")), 1)
            .await?;
        let updated = platform
            .update_git_connection(entry.id, request("renamed", None), 2)
            .await?;
        assert_eq!(updated.name.as_str(), "renamed");
        assert_eq!(updated.updated_at, 2);
        let stored = platform
            .store
            .git_connection_secrets(entry.id)
            .await?
            .context("secrets")?;
        let aad = connection_aad(entry.id, "client_secret");
        assert_eq!(platform.keys.open(&aad, &stored.client_secret)?, b"s3cr3t");

        let deleted = platform.delete_git_connection(entry.id).await?;
        assert_eq!(deleted.id, entry.id);
        assert!(platform.git_connections().await?.is_empty());
        let missing = platform.delete_git_connection(entry.id).await;
        assert!(matches!(
            api_error(missing.unwrap_err()),
            Error::IdNotFound { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn a_new_client_disconnects_the_connection() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let entry = platform
            .add_git_connection(request("team", Some("s3cr3t")), 1)
            .await?;
        let tokens = StoredTokens {
            access_token: &[1],
            refresh_token: None,
            expires_at: None,
        };
        platform
            .store
            .connect_git_connection(entry.id, "alice", &tokens)
            .await?;
        let same = platform
            .update_git_connection(entry.id, request("team", None), 2)
            .await?;
        assert_eq!(same.status, ConnectionStatus::Connected);
        assert_eq!(same.account.as_deref(), Some("alice"));

        let mut other = request("team", None);
        other.client_id = "another".into();
        let changed = platform.update_git_connection(entry.id, other, 3).await?;
        assert_eq!(changed.status, ConnectionStatus::NotConnected);
        assert_eq!(changed.account, None);
        let stored = platform
            .store
            .git_connection_secrets(entry.id)
            .await?
            .context("secrets")?;
        assert_eq!(stored.access_token, None);
        Ok(())
    }

    #[tokio::test]
    async fn the_public_address_is_a_setting() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        assert_eq!(
            platform.platform_settings().await?,
            PlatformSettings::default()
        );
        let settings = PlatformSettings {
            public_url: Some("https://deploy.example.com".parse()?),
        };
        platform.set_platform_settings(&settings).await?;
        assert_eq!(platform.platform_settings().await?, settings);
        platform
            .set_platform_settings(&PlatformSettings::default())
            .await?;
        assert_eq!(platform.platform_settings().await?.public_url, None);
        Ok(())
    }
}
