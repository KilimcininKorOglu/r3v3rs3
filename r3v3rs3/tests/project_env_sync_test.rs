//! Integration test for cross-environment diff & sync in projects.

use r3v3rs3::server::Server;
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::Role;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    EnvDiffStatus, EnvEntry, EnvironmentDiffReport, EnvironmentEnvSpec, EnvironmentSyncMode,
    EnvironmentSyncReport, EnvironmentSyncRequest, Project, ProjectSpec,
};
use reqwest::header::COOKIE;
use reqwest::{Client, StatusCode};
use serde::Serialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{TestStorage, admin_session_cookie, alloc_tcp_port, wait_for_listener};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> anyhow::Result<Self> {
        let unique = hex::encode(rand::random::<[u8; 8]>());
        let path = std::env::temp_dir().join(format!("r3v3rs3-{name}-{unique}"));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn with_admin<F, O>(dir: &Path, func: F) -> anyhow::Result<()>
where
    F: FnOnce(SocketAddr) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
    let mut config = AppConfig::default();
    config.platform.enabled = true;
    let storage = TestStorage::builder()
        .config(config)
        .account("admin", "secret", Role::Admin, None)
        .build();
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;
    let addr = alloc_tcp_port().await?.socket_addr();
    let (server, channels) = Server::new(new_appinfo(dir, dir), storage).await;
    let event = channels.event.clone();
    let task = tokio::spawn(server.start());
    tokio::spawn(start_admin(
        new_appinfo(dir, dir),
        addr,
        channels.command,
        channels.callback,
        channels.event.clone(),
        channels.accounts.clone(),
    ));
    wait_for_listener(addr).await?;
    let result = func(addr).await;
    event.send(r3v3rs3_api::event::ServerEvent::Shutdown)?;
    task.await??;
    result
}

/// An admin API session: the client, the admin address and the session cookie.
struct Api {
    client: Client,
    addr: SocketAddr,
    cookie: String,
}

impl Api {
    /// The URL of an environment path under the project.
    fn env_url(&self, project: ShortId, path: &str) -> String {
        format!(
            "http://{}/api/projects/{project}/environments/{path}",
            self.addr
        )
    }

    /// Sends an authenticated GET.
    async fn get(&self, url: String) -> anyhow::Result<reqwest::Response> {
        Ok(self
            .client
            .get(url)
            .header(COOKIE, &self.cookie)
            .send()
            .await?)
    }

    /// Sends an authenticated request with a JSON body.
    async fn send_json<B: Serialize>(
        &self,
        method: reqwest::Method,
        url: String,
        body: &B,
    ) -> anyhow::Result<reqwest::Response> {
        Ok(self
            .client
            .request(method, url)
            .header(COOKIE, &self.cookie)
            .json(body)
            .send()
            .await?)
    }
}

/// A plain or secret environment variable.
fn env_entry(key: &str, value: &str, secret: bool) -> EnvEntry {
    EnvEntry {
        key: key.parse().unwrap(),
        value: Some(value.to_string()),
        secret,
    }
}

/// Returns the id of the environment with the name.
fn environment_id(project: &Project, name: &str) -> ShortId {
    project
        .environments
        .iter()
        .find(|e| e.name == name)
        .unwrap()
        .id
}

/// Creates the project with staging and production environments and returns it with the ids
/// of the staging and production environments.
async fn create_project(api: &Api) -> anyhow::Result<(Project, ShortId, ShortId)> {
    // 1. Create a project with staging and production environments
    let project_spec = ProjectSpec {
        name: "Platform Test Project".to_string(),
        description: Some("Project for testing diff and sync".to_string()),
        default_environments: vec!["staging".to_string(), "production".to_string()],
    };
    let url = format!("http://{}/api/projects", api.addr);
    let response = api
        .send_json(reqwest::Method::POST, url, &project_spec)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let project: Project = response.json().await?;
    assert_eq!(project.environments.len(), 2);

    let staging_id = environment_id(&project, "staging");
    let prod_id = environment_id(&project, "production");
    Ok((project, staging_id, prod_id))
}

/// Sets the variables of an environment and checks that the request succeeds.
async fn put_env(
    api: &Api,
    project: ShortId,
    env: ShortId,
    variables: Vec<EnvEntry>,
) -> anyhow::Result<()> {
    let url = api.env_url(project, &format!("{env}/env"));
    let spec = EnvironmentEnvSpec { variables };
    let response = api.send_json(reqwest::Method::PUT, url, &spec).await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

/// Sets the staging and the production variables.
async fn set_variables(
    api: &Api,
    project: ShortId,
    staging_id: ShortId,
    prod_id: ShortId,
) -> anyhow::Result<()> {
    // 2. Set variables in Staging
    let staging_vars = vec![
        env_entry("APP_NAME", "my-app", false),
        env_entry("DB_HOST", "staging-db.internal", false),
        env_entry("SECRET_TOKEN", "s3cr3t-staging-val", true),
    ];
    put_env(api, project, staging_id, staging_vars).await?;

    // 3. Set variables in Production
    let prod_vars = vec![
        env_entry("APP_NAME", "my-app", false),
        env_entry("DB_HOST", "prod-db.internal", false),
        env_entry("EXTRA_PROD", "prod-only-val", false),
    ];
    put_env(api, project, prod_id, prod_vars).await
}

/// Checks the diff from staging to production and that it hides the secret value.
async fn check_diff(
    api: &Api,
    project: ShortId,
    staging_id: ShortId,
    prod_id: ShortId,
) -> anyhow::Result<()> {
    // 4. Compare Staging -> Production
    let response = api
        .get(api.env_url(project, &format!("{staging_id}/diff/{prod_id}")))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let diff: EnvironmentDiffReport = response.json().await?;

    assert_eq!(diff.total_keys, 4);
    assert_eq!(diff.identical, 1); // APP_NAME
    assert_eq!(diff.different_values, 1); // DB_HOST
    assert_eq!(diff.missing_in_target, 1); // SECRET_TOKEN
    assert_eq!(diff.missing_in_source, 1); // EXTRA_PROD

    // Verify secrets are never exposed in diff response
    let secret_entry = diff
        .entries
        .iter()
        .find(|e| e.key.as_str() == "SECRET_TOKEN")
        .unwrap();
    assert_eq!(secret_entry.status, EnvDiffStatus::MissingInTarget);
    assert!(secret_entry.source_secret);
    assert!(secret_entry.source_value.is_none());
    Ok(())
}

/// Sends a sync request, checks that it succeeds and returns the report.
async fn sync(
    api: &Api,
    project: ShortId,
    request: &EnvironmentSyncRequest,
) -> anyhow::Result<EnvironmentSyncReport> {
    let url = api.env_url(project, "sync");
    let response = api.send_json(reqwest::Method::POST, url, request).await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(response.json().await?)
}

/// Checks that a sync of the missing keys adds the secret to production and hides its value.
async fn check_sync_missing(
    api: &Api,
    project: ShortId,
    staging_id: ShortId,
    prod_id: ShortId,
) -> anyhow::Result<()> {
    // 5. Sync Missing Keys from Staging to Production
    let sync_req = EnvironmentSyncRequest {
        source_env_id: staging_id,
        target_env_id: prod_id,
        mode: EnvironmentSyncMode::SyncMissing,
        keys: Vec::new(),
    };
    let sync_rep = sync(api, project, &sync_req).await?;
    assert_eq!(sync_rep.synced_count, 1);
    assert_eq!(sync_rep.added_keys.len(), 1);
    assert_eq!(sync_rep.added_keys[0].as_str(), "SECRET_TOKEN");

    // 6. Verify Production variables now include SECRET_TOKEN
    let response = api
        .get(api.env_url(project, &format!("{prod_id}/env")))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let updated_prod_vars: Vec<EnvEntry> = response.json().await?;
    assert_eq!(updated_prod_vars.len(), 4);
    let prod_token = updated_prod_vars
        .iter()
        .find(|e| e.key.as_str() == "SECRET_TOKEN")
        .unwrap();
    assert!(prod_token.secret);
    assert!(prod_token.value.is_none());
    Ok(())
}

/// Checks that a sync of a selected key updates its value in production.
async fn check_sync_selected(
    api: &Api,
    project: ShortId,
    staging_id: ShortId,
    prod_id: ShortId,
) -> anyhow::Result<()> {
    // 7. Sync Selected Key (DB_HOST) from Staging to Production
    let sync_selected = EnvironmentSyncRequest {
        source_env_id: staging_id,
        target_env_id: prod_id,
        mode: EnvironmentSyncMode::SelectedKeys,
        keys: vec!["DB_HOST".parse().unwrap()],
    };
    let sync_rep2 = sync(api, project, &sync_selected).await?;
    assert_eq!(sync_rep2.synced_count, 1);
    assert_eq!(sync_rep2.updated_keys.len(), 1);
    assert_eq!(sync_rep2.updated_keys[0].as_str(), "DB_HOST");

    // Verify Production DB_HOST was updated
    let response = api
        .get(api.env_url(project, &format!("{prod_id}/env")))
        .await?;
    let final_prod_vars: Vec<EnvEntry> = response.json().await?;
    let prod_db_host = final_prod_vars
        .iter()
        .find(|e| e.key.as_str() == "DB_HOST")
        .unwrap();
    assert_eq!(prod_db_host.value.as_deref(), Some("staging-db.internal"));
    Ok(())
}

/// Runs the diff and sync steps against the admin API.
async fn run_diff_and_sync(addr: SocketAddr) -> anyhow::Result<()> {
    let api = Api {
        client: Client::new(),
        addr,
        cookie: admin_session_cookie(addr).await?,
    };
    let (project, staging_id, prod_id) = create_project(&api).await?;
    set_variables(&api, project.id, staging_id, prod_id).await?;
    check_diff(&api, project.id, staging_id, prod_id).await?;
    check_sync_missing(&api, project.id, staging_id, prod_id).await?;
    check_sync_selected(&api, project.id, staging_id, prod_id).await
}

#[tokio::test]
async fn cross_environment_diff_and_sync() -> anyhow::Result<()> {
    let dir = TempDir::new("project-env-sync")?;
    with_admin(&dir.0, run_diff_and_sync).await
}
