//! The repositories and the branches that a connected Git provider connection reads for the app
//! form.

use super::Platform;
use super::oauth::{provider_failed, site};
use super::provider::NewHook;
use r3v3rs3_api::git_connection::{GitRepository, RepoName, RepositoryQuery};
use r3v3rs3_api::id::ShortId;

impl Platform {
    /// One page of the repositories that the account of the connection can read.
    pub async fn git_repositories(
        &self,
        id: ShortId,
        query: &RepositoryQuery,
    ) -> anyhow::Result<Vec<GitRepository>> {
        let (entry, token) = self.git_access_token(id).await?;
        let page = query.page.unwrap_or(1).max(1);
        site(&entry)
            .repositories(self.http().await?, &token, query.search.as_deref(), page)
            .await
            .map_err(provider_failed)
    }

    /// The branch names of a repository of the connection.
    pub async fn git_branches(
        &self,
        id: ShortId,
        repository: &RepoName,
    ) -> anyhow::Result<Vec<String>> {
        let (entry, token) = self.git_access_token(id).await?;
        site(&entry)
            .branches(self.http().await?, &token, repository)
            .await
            .map_err(provider_failed)
    }

    /// Adds a webhook to a repository of the connection that sends its push events to `url`
    /// signed with `secret`, and returns the webhook id of the provider.
    pub async fn create_git_hook(
        &self,
        id: ShortId,
        repository: &RepoName,
        url: &str,
        secret: &str,
    ) -> anyhow::Result<String> {
        let (entry, token) = self.git_access_token(id).await?;
        let hook = NewHook { url, secret };
        site(&entry)
            .create_hook(self.http().await?, &token, repository, &hook)
            .await
            .map_err(provider_failed)
    }

    /// Deletes a webhook of a repository of the connection.
    pub async fn delete_git_hook(
        &self,
        id: ShortId,
        repository: &RepoName,
        hook: &str,
    ) -> anyhow::Result<()> {
        let (entry, token) = self.git_access_token(id).await?;
        site(&entry)
            .delete_hook(self.http().await?, &token, repository, hook)
            .await
            .map_err(provider_failed)
    }
}
