//! The repositories, the branches and the webhooks of an account at GitHub, GitLab and Gitea.

use super::Site;
use crate::cdn::fetch::HttpClient;
use crate::certs::dns::api::{ApiClient, ApiRequest};
use anyhow::{anyhow, bail};
use hyper::{Method, StatusCode};
use r3v3rs3_api::git_connection::{GitProvider, GitRepository, RepoName};
use serde_json::{Value, json};

/// The repositories of one page.
const PAGE_SIZE: u32 = 50;

/// The pages that a GitHub search reads, because the repository list of GitHub has no search.
const GITHUB_SEARCH_PAGES: u32 = 10;

/// The branches that a list returns.
const BRANCH_LIMIT: u32 = 50;

/// A webhook of a repository that sends its push events to an app.
pub(in crate::platform) struct NewHook<'a> {
    pub url: &'a str,
    pub secret: &'a str,
}

impl Site<'_> {
    /// One page of the repositories that the account can read, the recently changed first. With
    /// `search` the list holds the repositories whose name contains it.
    pub async fn repositories(
        &self,
        http: &HttpClient,
        token: &str,
        search: Option<&str>,
        page: u32,
    ) -> anyhow::Result<Vec<GitRepository>> {
        let api = self.api(http)?;
        let search = search.map(str::trim).filter(|search| !search.is_empty());
        match (self.provider, search) {
            (GitProvider::Github, Some(search)) => github_search(&api, token, search).await,
            (GitProvider::Github, None) => github_page(&api, token, page).await,
            (GitProvider::Gitlab, _) => gitlab_page(&api, token, search, page).await,
            (GitProvider::Gitea, _) => gitea_page(&api, token, search, page).await,
        }
    }

    /// The branch names of a repository.
    pub async fn branches(
        &self,
        http: &HttpClient,
        token: &str,
        repository: &RepoName,
    ) -> anyhow::Result<Vec<String>> {
        let repo = self.repo_path(repository);
        let path = match self.provider {
            GitProvider::Github => format!("{repo}/branches?per_page={BRANCH_LIMIT}"),
            GitProvider::Gitlab => format!("{repo}/repository/branches?per_page={BRANCH_LIMIT}"),
            GitProvider::Gitea => format!("{repo}/branches?limit={BRANCH_LIMIT}"),
        };
        let branches = self.api(http)?.json(get(path, token)).await?;
        Ok(names(&branches))
    }

    /// Adds a webhook that sends the push events of the repository, and returns its id.
    pub async fn create_hook(
        &self,
        http: &HttpClient,
        token: &str,
        repository: &RepoName,
        hook: &NewHook<'_>,
    ) -> anyhow::Result<String> {
        let path = format!("{}/hooks", self.repo_path(repository));
        let request = ApiRequest::new(Method::POST, path)
            .bearer(token)
            .json(&self.hook_body(hook));
        let created = self.api(http)?.json(request).await?;
        crate::certs::dns::api::id_text(&created["id"])
            .ok_or_else(|| anyhow!("the provider returned no webhook id"))
    }

    /// Deletes a webhook. A webhook that is gone already is not an error.
    pub async fn delete_hook(
        &self,
        http: &HttpClient,
        token: &str,
        repository: &RepoName,
        id: &str,
    ) -> anyhow::Result<()> {
        if !id.chars().all(|c| c.is_ascii_digit()) {
            bail!("invalid webhook id {id}");
        }
        let path = format!("{}/hooks/{id}", self.repo_path(repository));
        let request = ApiRequest::new(Method::DELETE, path).bearer(token);
        let (target, status, _) = self.api(http)?.exchange(request).await?;
        if status.is_success() || status == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(anyhow!("{target} returned {status}"))
        }
    }

    /// The API path of a repository. GitLab names a project by its encoded full name.
    fn repo_path(&self, repository: &RepoName) -> String {
        match self.provider {
            GitProvider::Gitlab => format!("/projects/{}", repository.as_str().replace('/', "%2F")),
            GitProvider::Github | GitProvider::Gitea => format!("/repos/{repository}"),
        }
    }

    fn hook_body(&self, hook: &NewHook<'_>) -> Value {
        match self.provider {
            GitProvider::Github => json!({
                "name": "web",
                "active": true,
                "events": ["push"],
                "config": {"url": hook.url, "content_type": "json", "secret": hook.secret, "insecure_ssl": "0"},
            }),
            GitProvider::Gitlab => json!({
                "url": hook.url,
                "token": hook.secret,
                "push_events": true,
                "tag_push_events": true,
                "enable_ssl_verification": true,
            }),
            GitProvider::Gitea => json!({
                "type": "gitea",
                "active": true,
                "events": ["push"],
                "config": {"url": hook.url, "content_type": "json", "secret": hook.secret},
            }),
        }
    }
}

fn get(path: String, token: &str) -> ApiRequest {
    ApiRequest::new(Method::GET, path).bearer(token)
}

/// The `name` field of every item.
fn names(items: &Value) -> Vec<String> {
    crate::certs::dns::api::strings(items, "name")
}

/// Percent-encodes a search term for a query.
fn encoded(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

async fn github_page(
    api: &ApiClient,
    token: &str,
    page: u32,
) -> anyhow::Result<Vec<GitRepository>> {
    let path = format!(
        "/user/repos?sort=updated&affiliation=owner,collaborator,organization_member&per_page={PAGE_SIZE}&page={page}"
    );
    let items = api.json(get(path, token)).await?;
    Ok(repositories(&items, github_repository))
}

/// Reads the first pages and keeps the repositories whose name contains `search`.
async fn github_search(
    api: &ApiClient,
    token: &str,
    search: &str,
) -> anyhow::Result<Vec<GitRepository>> {
    let search = search.to_lowercase();
    let mut found = Vec::new();
    for page in 1..=GITHUB_SEARCH_PAGES {
        let items = github_page(api, token, page).await?;
        let last = items.len() < PAGE_SIZE as usize;
        found.extend(
            items
                .into_iter()
                .filter(|repo| repo.full_name.as_str().to_lowercase().contains(&search)),
        );
        if last || found.len() >= PAGE_SIZE as usize {
            break;
        }
    }
    found.truncate(PAGE_SIZE as usize);
    Ok(found)
}

async fn gitlab_page(
    api: &ApiClient,
    token: &str,
    search: Option<&str>,
    page: u32,
) -> anyhow::Result<Vec<GitRepository>> {
    let mut path = format!(
        "/projects?membership=true&simple=true&order_by=last_activity_at&per_page={PAGE_SIZE}&page={page}"
    );
    if let Some(search) = search {
        path.push_str(&format!("&search={}", encoded(search)));
    }
    let items = api.json(get(path, token)).await?;
    Ok(repositories(&items, gitlab_repository))
}

/// Gitea searches the repositories that the user owns or contributes to, so the list needs the id
/// of the user.
async fn gitea_page(
    api: &ApiClient,
    token: &str,
    search: Option<&str>,
    page: u32,
) -> anyhow::Result<Vec<GitRepository>> {
    let user = api.json(get("/user".to_string(), token)).await?;
    let uid = user["id"]
        .as_u64()
        .ok_or_else(|| anyhow!("the provider returned no user id"))?;
    let query = encoded(search.unwrap_or_default());
    let path = format!(
        "/repos/search?uid={uid}&sort=updated&order=desc&limit={PAGE_SIZE}&page={page}&q={query}"
    );
    let found = api.json(get(path, token)).await?;
    Ok(repositories(&found["data"], github_repository))
}

/// The repositories of a JSON array. An item without a valid name is left out.
fn repositories(items: &Value, of: fn(&Value) -> Option<GitRepository>) -> Vec<GitRepository> {
    items
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(of)
        .collect()
}

/// A repository of GitHub or Gitea, which use the same fields.
fn github_repository(item: &Value) -> Option<GitRepository> {
    Some(GitRepository {
        full_name: item["full_name"].as_str()?.parse().ok()?,
        clone_url: item["clone_url"].as_str()?.to_string(),
        default_branch: item["default_branch"].as_str().map(str::to_string),
        private: item["private"].as_bool().unwrap_or(false),
    })
}

fn gitlab_repository(item: &Value) -> Option<GitRepository> {
    Some(GitRepository {
        full_name: item["path_with_namespace"].as_str()?.parse().ok()?,
        clone_url: item["http_url_to_repo"].as_str()?.to_string(),
        default_branch: item["default_branch"].as_str().map(str::to_string),
        private: item["visibility"].as_str() != Some("public"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;
    use r3v3rs3_api::git_connection::ProviderUrl;

    async fn http() -> anyhow::Result<HttpClient> {
        crate::cdn::fetch::build_client().await
    }

    #[tokio::test]
    async fn each_provider_lists_its_repositories() -> anyhow::Result<()> {
        let mut server = mockito::Server::new_async().await;
        let url: ProviderUrl = server.url().parse()?;
        let http = http().await?;
        let gitlab = server
            .mock("GET", "/api/v4/projects")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("membership".into(), "true".into()),
                Matcher::UrlEncoded("search".into(), "sh op".into()),
                Matcher::UrlEncoded("page".into(), "2".into()),
            ]))
            .match_header("authorization", "Bearer t")
            .with_body(
                r#"[{"path_with_namespace":"team/sub/shop","http_url_to_repo":"https://g.example/team/sub/shop.git","default_branch":"main","visibility":"private"},{"path_with_namespace":"bad name"}]"#,
            )
            .create_async()
            .await;
        let site = Site {
            provider: GitProvider::Gitlab,
            url: &url,
        };
        let found = site.repositories(&http, "t", Some(" sh op "), 2).await?;
        gitlab.assert_async().await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].full_name.as_str(), "team/sub/shop");
        assert!(found[0].private);

        server
            .mock("GET", "/api/v1/user")
            .with_body(r#"{"id":7,"login":"alice"}"#)
            .create_async()
            .await;
        let gitea = server
            .mock("GET", "/api/v1/repos/search")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("uid".into(), "7".into()),
                Matcher::UrlEncoded("q".into(), "".into()),
            ]))
            .with_body(r#"{"ok":true,"data":[{"full_name":"alice/app","clone_url":"https://g.example/alice/app.git","private":false}]}"#)
            .create_async()
            .await;
        let site = Site {
            provider: GitProvider::Gitea,
            url: &url,
        };
        let found = site.repositories(&http, "t", None, 1).await?;
        gitea.assert_async().await;
        assert_eq!(found[0].full_name.as_str(), "alice/app");
        assert_eq!(found[0].default_branch, None);
        Ok(())
    }

    #[tokio::test]
    async fn a_github_search_filters_the_pages() -> anyhow::Result<()> {
        let mut server = mockito::Server::new_async().await;
        let url: ProviderUrl = server.url().parse()?;
        let repo = |name: &str| json!({"full_name": name, "clone_url": format!("https://github.com/{name}.git"), "private": true});
        let first = (0..PAGE_SIZE)
            .map(|index| repo(&format!("o/other-{index}")))
            .collect::<Vec<_>>();
        server
            .mock("GET", "/api/v3/user/repos")
            .match_query(Matcher::UrlEncoded("page".into(), "1".into()))
            .with_body(Value::Array(first).to_string())
            .create_async()
            .await;
        server
            .mock("GET", "/api/v3/user/repos")
            .match_query(Matcher::UrlEncoded("page".into(), "2".into()))
            .with_body(json!([repo("o/Shop"), repo("o/blog")]).to_string())
            .create_async()
            .await;
        let site = Site {
            provider: GitProvider::Github,
            url: &url,
        };
        let found = site
            .repositories(&http().await?, "t", Some("shop"), 1)
            .await?;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].full_name.as_str(), "o/Shop");
        Ok(())
    }

    #[tokio::test]
    async fn a_hook_is_created_and_deleted_on_each_provider() -> anyhow::Result<()> {
        let mut server = mockito::Server::new_async().await;
        let url: ProviderUrl = server.url().parse()?;
        let http = http().await?;
        let repository: RepoName = "team/sub/shop".parse()?;
        let hook = NewHook {
            url: "https://deploy.example.com/hooks/apps/bcd-fgh",
            secret: "s3cr3t",
        };
        let cases = [
            (
                GitProvider::Github,
                "/api/v3/repos/team/sub/shop/hooks",
                json!({"config": {"secret": "s3cr3t", "content_type": "json"}, "events": ["push"]}),
            ),
            (
                GitProvider::Gitlab,
                "/api/v4/projects/team%2Fsub%2Fshop/hooks",
                json!({"token": "s3cr3t", "push_events": true}),
            ),
            (
                GitProvider::Gitea,
                "/api/v1/repos/team/sub/shop/hooks",
                json!({"type": "gitea", "config": {"secret": "s3cr3t"}}),
            ),
        ];
        for (provider, path, body) in cases {
            let create = server
                .mock("POST", path)
                .match_body(Matcher::PartialJson(body))
                .with_status(201)
                .with_body(r#"{"id":42}"#)
                .create_async()
                .await;
            let delete = server
                .mock("DELETE", format!("{path}/42").as_str())
                .with_status(404)
                .create_async()
                .await;
            let site = Site {
                provider,
                url: &url,
            };
            assert_eq!(
                site.create_hook(&http, "t", &repository, &hook).await?,
                "42"
            );
            site.delete_hook(&http, "t", &repository, "42").await?;
            create.assert_async().await;
            delete.assert_async().await;
        }
        let site = Site {
            provider: GitProvider::Gitea,
            url: &url,
        };
        assert!(
            site.delete_hook(&http, "t", &repository, "1/../2")
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_branches_come_from_the_repository() -> anyhow::Result<()> {
        let mut server = mockito::Server::new_async().await;
        let url: ProviderUrl = server.url().parse()?;
        let branches = server
            .mock("GET", "/api/v4/projects/team%2Fshop/repository/branches")
            .match_query(Matcher::UrlEncoded("per_page".into(), "50".into()))
            .with_body(r#"[{"name":"main"},{"name":"dev"}]"#)
            .create_async()
            .await;
        let site = Site {
            provider: GitProvider::Gitlab,
            url: &url,
        };
        let names = site
            .branches(&http().await?, "t", &"team/shop".parse()?)
            .await?;
        branches.assert_async().await;
        assert_eq!(names, ["main", "dev"]);
        Ok(())
    }
}
