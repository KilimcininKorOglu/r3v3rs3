//! The deployment of an app on an agent target, with an agent in memory.

use super::Platform;
use super::fake::{COMMIT, FakeFetcher, FakeRuntime};
use super::tests::{TempDir, api_error, platform_with, request};
use crate::agent::forward::ForwardKey;
use crate::agent::testing::TestAgent;
use crate::command::ServerCommand;
use anyhow::{Context as _, bail};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppRequest, AppSource, DeploymentEntry, DeploymentStatus, DeploymentTrigger,
    LOCAL_TARGET, PlatformConfig,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

struct Setup {
    platform: Arc<Platform>,
    local: Arc<FakeRuntime>,
    agent: TestAgent,
    app: AppEntry,
    commands: mpsc::Receiver<ServerCommand>,
    _dir: TempDir,
}

/// An app server on 127.0.0.1 of the agent host that answers every request with 200.
async fn app_server() -> anyhow::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
            });
        }
    });
    Ok(port)
}

/// A platform with the agent target `edge`, its connected agent and the image app `shop` on it.
async fn setup() -> anyhow::Result<Setup> {
    setup_with(request("shop")).await
}

/// A platform with the agent target `edge`, its connected agent and the app of `shop` on it.
async fn setup_with(mut shop: AppRequest) -> anyhow::Result<Setup> {
    let config = PlatformConfig {
        proxy_ports: vec!["http".into()],
        ..Default::default()
    };
    // Each deployment sends a snapshot and two notifications, and the tests read them later.
    let (command, commands) = mpsc::channel(64);
    let (mut platform, local, dir) = platform_with(&config, command).await?;
    platform.fetcher = Arc::new(FakeFetcher::default());
    let edge: ShortId = "fzn-txd".parse()?;
    platform
        .store
        .add_agent_target(edge, "edge", "hash", 1)
        .await?;
    let agent = TestAgent::connect(&platform.agents, edge);
    agent.runtime.state().host_port = app_server().await?;
    shop.target = edge;
    shop.spec.domains = vec!["shop.example.com".parse()?];
    shop.spec.health_check_path = Some("/healthz".into());
    let app = platform.add_app(shop, 1).await?;
    Ok(Setup {
        platform: Arc::new(platform),
        local,
        agent,
        app,
        commands,
        _dir: dir,
    })
}

async fn deploy(setup: &Setup) -> anyhow::Result<DeploymentEntry> {
    let entry = setup
        .platform
        .deploy(setup.app.id, "alice", DeploymentTrigger::Manual)
        .await?;
    for _ in 0..500 {
        if !setup.platform.busy_apps().contains(&setup.app.id) {
            return setup.platform.deployment(entry.id).await;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    bail!("the deployment {} did not end", entry.id)
}

/// The port of the first route server of the last proxy snapshot. The notifications between the
/// snapshots do not matter here.
fn route_port(commands: &mut mpsc::Receiver<ServerCommand>) -> anyhow::Result<u16> {
    let mut last = None;
    while let Ok(command) = commands.try_recv() {
        if let ServerCommand::SetDiscovery { snapshot } = command {
            last = Some(snapshot);
        }
    }
    let snapshot = last.context("no snapshot")?;
    let proxies = snapshot.proxies.context("proxies")?;
    let servers = crate::discovery::first_route_servers(proxies.first().context("proxy")?);
    let server = servers.first().context("server")?;
    let port = server
        .trim_start_matches("http://127.0.0.1:")
        .trim_end_matches('/');
    Ok(port.parse()?)
}

async fn get(port: u16) -> anyhow::Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: shop.example.com\r\nConnection: close\r\n\r\n")
        .await?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await?;
    Ok(answer)
}

#[tokio::test]
async fn an_app_on_an_agent_runs_there_and_serves_through_a_forwarder() -> anyhow::Result<()> {
    let mut setup = setup().await?;
    let deployment = deploy(&setup).await?;
    assert_eq!(
        deployment.status,
        DeploymentStatus::Running,
        "{deployment:?}"
    );
    let container = format!("r3v3rs3-{}-{}", setup.app.id, deployment.id);
    assert_eq!(
        setup.agent.runtime.names(),
        std::slice::from_ref(&container)
    );
    assert!(setup.local.names().is_empty());

    assert!(setup.platform.publish().await);
    let port = route_port(&mut setup.commands)?;
    assert_ne!(port, setup.agent.runtime.state().host_port);
    assert!(get(port).await?.starts_with("HTTP/1.1 200"));

    let log = setup.platform.app_log(setup.app.id, 10).await?;
    assert_eq!(log.log, format!("the log of {container}\n"));
    Ok(())
}

#[tokio::test]
async fn an_offline_agent_keeps_the_route_and_fails_new_deployments() -> anyhow::Result<()> {
    let mut setup = setup().await?;
    deploy(&setup).await?;
    assert!(setup.platform.publish().await);
    let port = route_port(&mut setup.commands)?;
    setup.agent.stop().await;

    // The snapshot does not change, and the forwarder closes every connection.
    assert!(setup.platform.publish().await);
    assert!(setup.commands.try_recv().is_err());
    // The forwarder closes the connection, which the client can see as a reset.
    let answer = get(port).await;
    assert!(answer.as_ref().map_or(true, String::is_empty), "{answer:?}");

    let err = setup.platform.app_log(setup.app.id, 10).await.unwrap_err();
    assert!(matches!(api_error(err), Error::AgentOffline { .. }));
    let failed = deploy(&setup).await?;
    assert_eq!(failed.status, DeploymentStatus::Failed);
    let message = failed.message.context("message")?;
    assert!(message.contains("not connected"), "{message}");
    Ok(())
}

#[tokio::test]
async fn a_deleted_app_leaves_nothing_on_the_agent() -> anyhow::Result<()> {
    let setup = setup().await?;
    let deployment = deploy(&setup).await?;
    let key = ForwardKey {
        target: setup.app.target,
        container: format!("r3v3rs3-{}-{}", setup.app.id, deployment.id).parse()?,
        port: setup.app.spec.port,
    };
    assert!(setup.platform.forwarders.existing(&key).is_some());

    // The containers stay on the target of their deployments.
    let mut moved = request("shop");
    moved.target = LOCAL_TARGET.parse()?;
    let err = setup.platform.update_app(setup.app.id, moved, 2).await;
    assert!(matches!(
        api_error(err.unwrap_err()),
        Error::AppTargetFixed { .. }
    ));

    setup.platform.delete_app(setup.app.id).await?;
    assert!(setup.agent.runtime.names().is_empty());
    assert!(setup.agent.runtime.state().networks.is_empty());
    assert_eq!(setup.platform.forwarders.existing(&key), None);
    Ok(())
}

#[tokio::test]
async fn a_rollback_on_an_agent_starts_the_old_image_there() -> anyhow::Result<()> {
    let setup = setup().await?;
    let first = deploy(&setup).await?;
    deploy(&setup).await?;
    let rollback = setup.platform.rollback(first.id, "bob").await?;
    for _ in 0..500 {
        if !setup.platform.busy_apps().contains(&setup.app.id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let rollback = setup.platform.deployment(rollback.id).await?;
    assert_eq!(rollback.status, DeploymentStatus::Running, "{rollback:?}");
    assert_eq!(rollback.image_digest, first.image_digest);
    let container = format!("r3v3rs3-{}-{}", setup.app.id, rollback.id);
    assert_eq!(setup.agent.runtime.names(), [container]);
    Ok(())
}

fn compose_request() -> anyhow::Result<AppRequest> {
    let mut stack = request("stack");
    stack.spec.source = AppSource::Compose {
        repository: "https://git.example.com/team/stack.git".parse()?,
        branch: "release".parse()?,
        file: None,
        service: "web".parse()?,
    };
    Ok(stack)
}

#[tokio::test]
async fn a_compose_app_on_an_agent_runs_from_its_files_there() -> anyhow::Result<()> {
    let setup = setup_with(compose_request()?).await?;
    let first = deploy(&setup).await?;
    assert_eq!(first.status, DeploymentStatus::Running, "{first:?}");
    assert_eq!(first.commit_sha.as_deref(), Some(COMMIT));
    let project = format!("r3v3rs3-{}", setup.app.id);
    let calls = [format!("config {project}"), format!("up {project}")];
    assert_eq!(setup.agent.compose.state().calls, calls);
    let container = format!("r3v3rs3-{}-{}", setup.app.id, first.id);
    assert_eq!(setup.agent.runtime.names(), [container]);
    let app_dir = setup.agent.dir.join(setup.app.id.to_string());
    let first_dir = app_dir.join(first.id.to_string());
    assert!(first_dir.join("src/compose.yaml").is_file());
    assert!(first_dir.join(super::compose::OVERRIDE_FILE).is_file());

    let second = deploy(&setup).await?;
    assert_eq!(second.status, DeploymentStatus::Running, "{second:?}");
    assert!(!first_dir.exists());
    assert!(app_dir.join(second.id.to_string()).is_dir());

    setup.platform.delete_app(setup.app.id).await?;
    let calls = setup.agent.compose.state().calls.clone();
    assert_eq!(calls.last(), Some(&format!("down {project}")));
    assert!(!app_dir.exists());
    let master_dir = setup.platform.compose_dir.join(setup.app.id.to_string());
    assert!(!master_dir.exists());
    Ok(())
}

#[tokio::test]
async fn a_git_app_on_an_agent_is_built_there() -> anyhow::Result<()> {
    let mut shop = request("shop");
    shop.spec.source = AppSource::Git {
        repository: "https://git.example.com/team/shop.git".parse()?,
        branch: "main".parse()?,
        context: "app".parse()?,
        dockerfile: "Dockerfile".parse()?,
    };
    let setup = setup_with(shop).await?;
    let deployment = deploy(&setup).await?;
    assert_eq!(
        deployment.status,
        DeploymentStatus::Running,
        "{deployment:?}"
    );
    let builds = setup
        .agent
        .runtime
        .state()
        .builds
        .iter()
        .map(|build| build.tag.clone())
        .collect::<Vec<_>>();
    let tag = format!("r3v3rs3/{}:{}", setup.app.id, deployment.id);
    assert_eq!(builds, [tag]);
    assert!(setup.local.state().builds.is_empty());
    Ok(())
}
