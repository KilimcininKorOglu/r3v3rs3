//! Integration test for Docker Compose .env template resolver & secret generator.

use r3v3rs3::server::Server;
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::Role;
use r3v3rs3_api::compose_env::{
    GenerateAppEnvSecretsRequest, ResolveEnvTemplateRequest, ResolveEnvTemplateResponse,
};
use r3v3rs3_api::platform::{AppEntry, AppRequest, AppSource, AppSpec, EnvEntry};
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
    O: std::future::Future<Output = anyhow::Result<()>>,
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
    event.send(r3v3rs3::server::ServerEvent::Shutdown)?;
    let _ = task.await;
    result
}

#[tokio::test]
async fn test_resolve_compose_env_template_endpoint() -> anyhow::Result<()> {
    let dir = TempDir::new("compose-env-test")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = admin_session_cookie(addr).await?;

        let template_input = r#"# Production Compose Env
APP_NAME=my-app
SITE_URL=https://example.com
POSTGRES_PASSWORD=
APP_KEY=changeme
JWT_SECRET=""
"#;

        let req = ResolveEnvTemplateRequest {
            template: Some(template_input.to_string()),
            entries: None,
            overwrite: false,
        };

        let res = client
            .post(format!("http://{addr}/api/compose/env/generate"))
            .header(COOKIE, &cookie)
            .json(&req)
            .send()
            .await?;

        assert_eq!(res.status(), StatusCode::OK);
        let body: ResolveEnvTemplateResponse = res.json().await?;

        assert_eq!(body.generated_count, 3);
        assert!(
            body.generated_keys
                .contains(&"POSTGRES_PASSWORD".to_string())
        );
        assert!(body.generated_keys.contains(&"APP_KEY".to_string()));
        assert!(body.generated_keys.contains(&"JWT_SECRET".to_string()));

        assert!(body.template.contains("APP_NAME=my-app"));
        assert!(body.template.contains("SITE_URL=https://example.com"));
        assert!(body.template.contains("POSTGRES_PASSWORD="));
        assert!(body.template.contains("APP_KEY="));
        assert!(body.template.contains("JWT_SECRET="));

        // Check entries
        let pg_entry = body
            .entries
            .iter()
            .find(|e| e.key.as_str() == "POSTGRES_PASSWORD")
            .unwrap();
        assert!(pg_entry.secret);
        assert_eq!(pg_entry.value.as_ref().unwrap().len(), 32);

        let key_entry = body
            .entries
            .iter()
            .find(|e| e.key.as_str() == "APP_KEY")
            .unwrap();
        assert!(key_entry.secret);
        assert_eq!(key_entry.value.as_ref().unwrap().len(), 64);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_generate_app_env_secrets_endpoint() -> anyhow::Result<()> {
    let dir = TempDir::new("app-env-gen-test")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = admin_session_cookie(addr).await?;

        // 1. Create an app
        let app_req = AppRequest {
            name: "test-app".parse()?,
            spec: AppSpec {
                port: 8080,
                source: AppSource::Image {
                    image: "nginx:alpine".parse()?,
                },
                ..Default::default()
            },
        };

        let res = client
            .post(format!("http://{addr}/api/apps"))
            .header(COOKIE, &cookie)
            .json(&app_req)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let app: AppEntry = res.json().await?;

        // 2. Set initial env with an empty secret variable
        let initial_env = vec![
            EnvEntry {
                key: "DATABASE_PASSWORD".parse()?,
                value: Some("".into()),
                secret: true,
            },
            EnvEntry {
                key: "NORMAL_VAR".parse()?,
                value: Some("hello".into()),
                secret: false,
            },
        ];

        let res = client
            .put(format!("http://{addr}/api/apps/{}/env", app.id))
            .header(COOKIE, &cookie)
            .json(&initial_env)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);

        // 3. Call generate secrets with save: true
        let gen_req = GenerateAppEnvSecretsRequest {
            save: true,
            overwrite: false,
        };

        let res = client
            .post(format!("http://{addr}/api/apps/{}/env/generate", app.id))
            .header(COOKIE, &cookie)
            .json(&gen_req)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let gen_res: ResolveEnvTemplateResponse = res.json().await?;

        assert_eq!(gen_res.generated_count, 1);
        assert_eq!(gen_res.generated_keys, vec!["DATABASE_PASSWORD"]);

        // 4. Verify stored env has the secret masked but stored
        let res = client
            .get(format!("http://{addr}/api/apps/{}/env", app.id))
            .header(COOKIE, &cookie)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let envs: Vec<EnvEntry> = res.json().await?;
        let db_pass = envs
            .iter()
            .find(|e| e.key.as_str() == "DATABASE_PASSWORD")
            .unwrap();
        assert!(db_pass.secret);
        assert!(db_pass.value.is_none()); // Masked in GET /env

        Ok(())
    })
    .await
}
