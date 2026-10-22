//! Integration test for cross-environment diff & sync in projects.

use r3v3rs3::server::Server;
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::Role;
use r3v3rs3_api::platform::{
    EnvDiffStatus, EnvEntry, EnvironmentDiffReport, EnvironmentEnvSpec, EnvironmentSyncMode,
    EnvironmentSyncReport, EnvironmentSyncRequest, Project, ProjectSpec,
};
use reqwest::header::COOKIE;
use reqwest::{Client, StatusCode};
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

#[tokio::test]
async fn cross_environment_diff_and_sync() -> anyhow::Result<()> {
    let dir = TempDir::new("project-env-sync")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = admin_session_cookie(addr).await?;

        // 1. Create a project with staging and production environments
        let project_spec = ProjectSpec {
            name: "Platform Test Project".to_string(),
            description: Some("Project for testing diff and sync".to_string()),
            default_environments: vec!["staging".to_string(), "production".to_string()],
        };
        let response = client
            .post(format!("http://{addr}/api/projects"))
            .header(COOKIE, &cookie)
            .json(&project_spec)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let project: Project = response.json().await?;
        assert_eq!(project.environments.len(), 2);

        let staging_id = project
            .environments
            .iter()
            .find(|e| e.name == "staging")
            .unwrap()
            .id;
        let prod_id = project
            .environments
            .iter()
            .find(|e| e.name == "production")
            .unwrap()
            .id;

        // 2. Set variables in Staging
        let staging_vars = EnvironmentEnvSpec {
            variables: vec![
                EnvEntry {
                    key: "APP_NAME".parse().unwrap(),
                    value: Some("my-app".to_string()),
                    secret: false,
                },
                EnvEntry {
                    key: "DB_HOST".parse().unwrap(),
                    value: Some("staging-db.internal".to_string()),
                    secret: false,
                },
                EnvEntry {
                    key: "SECRET_TOKEN".parse().unwrap(),
                    value: Some("s3cr3t-staging-val".to_string()),
                    secret: true,
                },
            ],
        };
        let response = client
            .put(format!(
                "http://{addr}/api/projects/{}/environments/{staging_id}/env",
                project.id
            ))
            .header(COOKIE, &cookie)
            .json(&staging_vars)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);

        // 3. Set variables in Production
        let prod_vars = EnvironmentEnvSpec {
            variables: vec![
                EnvEntry {
                    key: "APP_NAME".parse().unwrap(),
                    value: Some("my-app".to_string()),
                    secret: false,
                },
                EnvEntry {
                    key: "DB_HOST".parse().unwrap(),
                    value: Some("prod-db.internal".to_string()),
                    secret: false,
                },
                EnvEntry {
                    key: "EXTRA_PROD".parse().unwrap(),
                    value: Some("prod-only-val".to_string()),
                    secret: false,
                },
            ],
        };
        let response = client
            .put(format!(
                "http://{addr}/api/projects/{}/environments/{prod_id}/env",
                project.id
            ))
            .header(COOKIE, &cookie)
            .json(&prod_vars)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);

        // 4. Compare Staging -> Production
        let response = client
            .get(format!(
                "http://{addr}/api/projects/{}/environments/{staging_id}/diff/{prod_id}",
                project.id
            ))
            .header(COOKIE, &cookie)
            .send()
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

        // 5. Sync Missing Keys from Staging to Production
        let sync_req = EnvironmentSyncRequest {
            source_env_id: staging_id,
            target_env_id: prod_id,
            mode: EnvironmentSyncMode::SyncMissing,
            keys: Vec::new(),
        };
        let response = client
            .post(format!(
                "http://{addr}/api/projects/{}/environments/sync",
                project.id
            ))
            .header(COOKIE, &cookie)
            .json(&sync_req)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let sync_rep: EnvironmentSyncReport = response.json().await?;
        assert_eq!(sync_rep.synced_count, 1);
        assert_eq!(sync_rep.added_keys.len(), 1);
        assert_eq!(sync_rep.added_keys[0].as_str(), "SECRET_TOKEN");

        // 6. Verify Production variables now include SECRET_TOKEN
        let response = client
            .get(format!(
                "http://{addr}/api/projects/{}/environments/{prod_id}/env",
                project.id
            ))
            .header(COOKIE, &cookie)
            .send()
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

        // 7. Sync Selected Key (DB_HOST) from Staging to Production
        let sync_selected = EnvironmentSyncRequest {
            source_env_id: staging_id,
            target_env_id: prod_id,
            mode: EnvironmentSyncMode::SelectedKeys,
            keys: vec!["DB_HOST".parse().unwrap()],
        };
        let response = client
            .post(format!(
                "http://{addr}/api/projects/{}/environments/sync",
                project.id
            ))
            .header(COOKIE, &cookie)
            .json(&sync_selected)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let sync_rep2: EnvironmentSyncReport = response.json().await?;
        assert_eq!(sync_rep2.synced_count, 1);
        assert_eq!(sync_rep2.updated_keys.len(), 1);
        assert_eq!(sync_rep2.updated_keys[0].as_str(), "DB_HOST");

        // Verify Production DB_HOST was updated
        let response = client
            .get(format!(
                "http://{addr}/api/projects/{}/environments/{prod_id}/env",
                project.id
            ))
            .header(COOKIE, &cookie)
            .send()
            .await?;
        let final_prod_vars: Vec<EnvEntry> = response.json().await?;
        let prod_db_host = final_prod_vars
            .iter()
            .find(|e| e.key.as_str() == "DB_HOST")
            .unwrap();
        assert_eq!(prod_db_host.value.as_deref(), Some("staging-db.internal"));

        Ok(())
    })
    .await
}
