//! Integration test for the Compose architecture topology and service dependency graph API.

use r3v3rs3::server::Server;
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::Role;
use r3v3rs3_api::platform::{
    AppEntry, AppRequest, ComposeEdgeKind, ComposeNodeKind, ComposeTopology,
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

const SAMPLE_COMPOSE_YAML: &str = r#"
services:
  web:
    image: nginx:alpine
    ports:
      - "80:80"
    depends_on:
      api:
        condition: service_healthy
    networks:
      - frontend
      - backend
  api:
    image: node:20-alpine
    ports:
      - "3000:3000"
    depends_on:
      - db
      - cache
    networks:
      - backend
    volumes:
      - db_data:/app/data
  db:
    image: postgres:17-alpine
    networks:
      - backend
    volumes:
      - db_data:/var/lib/postgresql/data
  cache:
    image: redis:alpine
    networks:
      - backend

networks:
  frontend:
    driver: bridge
  backend:
    driver: bridge

volumes:
  db_data:
"#;

#[tokio::test]
async fn test_compose_architecture_topology_api() -> anyhow::Result<()> {
    let dir = TempDir::new("compose-topology")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = admin_session_cookie(addr).await?;
        let app = create_test_compose_app(&client, addr, &cookie).await?;

        let topo_res = client
            .get(format!("http://{addr}/api/apps/{}/topology", app.id))
            .header(COOKIE, &cookie)
            .send()
            .await?;
        assert_eq!(topo_res.status(), StatusCode::OK);
        let topology: ComposeTopology = topo_res.json().await?;

        assert_topology_summary_and_nodes(&topology);
        assert_topology_edges(&topology);

        Ok(())
    })
    .await
}

async fn create_test_compose_app(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
) -> anyhow::Result<AppEntry> {
    let app_req: AppRequest = serde_json::from_value(serde_json::json!({
        "name": "microservices-stack",
        "target": "local",
        "spec": {
            "domains": ["stack.example.com"],
            "port": 80,
            "source": {
                "type": "compose",
                "branch": "main",
                "service": "web",
                "compose_yaml": SAMPLE_COMPOSE_YAML
            }
        }
    }))?;

    let response = client
        .post(format!("http://{addr}/api/apps"))
        .header(COOKIE, cookie)
        .json(&app_req)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(response.json().await?)
}

fn assert_topology_summary_and_nodes(topology: &ComposeTopology) {
    assert_eq!(topology.summary.services_count, 4);
    assert_eq!(topology.summary.networks_count, 2);
    assert_eq!(topology.summary.volumes_count, 1);

    let web_node = topology
        .nodes
        .iter()
        .find(|n| n.name == "web" && n.kind == ComposeNodeKind::Service)
        .expect("web node must exist");
    let web_service = web_node.service.as_ref().expect("service details");
    assert!(web_service.is_entrypoint);
    assert_eq!(web_service.ports, vec!["80:80"]);
}

fn assert_topology_edges(topology: &ComposeTopology) {
    let web_to_api = topology
        .edges
        .iter()
        .find(|e| e.source == "service:web" && e.target == "service:api")
        .expect("web to api edge");
    assert_eq!(
        web_to_api.kind,
        ComposeEdgeKind::DependsOn {
            condition: Some("service_healthy".to_string())
        }
    );
    assert_eq!(web_to_api.label.as_deref(), Some("healthy"));

    assert!(
        topology
            .edges
            .iter()
            .any(|e| e.source == "service:api" && e.target == "service:db")
    );
    assert!(
        topology
            .edges
            .iter()
            .any(|e| e.source == "service:api" && e.target == "service:cache")
    );
    assert!(
        topology
            .edges
            .iter()
            .any(|e| e.source == "service:api" && e.target == "volume:db_data")
    );
    assert!(
        topology
            .edges
            .iter()
            .any(|e| e.source == "service:db" && e.target == "volume:db_data")
    );
}
