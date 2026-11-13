use axum::{Router, routing::get};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use r3v3rs3::server::{ServerChannels, rpc::config::SetConfig};
use r3v3rs3_api::{
    app::AppConfig,
    discovery::{DiscoveryState, DiscoveryStatus},
    error::Error,
};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::net::UnixListener;
use tokio::sync::broadcast;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};

mod common;
use common::{
    TestStorage, alloc_tcp_port, call, http_port_entry, serve_http_upstream, wait_for_discovery,
    wait_for_status, with_server,
};

type MockBody = BoxBody<Bytes, Infallible>;

/// A Docker Engine API on a Unix socket that serves a container list and sends one event for
/// each change of the list.
#[derive(Clone)]
struct MockDocker {
    containers: Arc<Mutex<Value>>,
    events: broadcast::Sender<()>,
}

impl MockDocker {
    fn set_containers(&self, containers: Value) {
        *self.containers.lock().unwrap() = containers;
        let _ = self.events.send(());
    }

    async fn serve(&self, path: &Path) -> anyhow::Result<()> {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        let mock = self.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let mock = mock.clone();
                let service =
                    hyper::service::service_fn(move |request| mock.clone().handle(request));
                tokio::spawn(async move {
                    let _ = auto::Builder::new(TokioExecutor::new())
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Ok(())
    }

    async fn handle(self, request: Request<Incoming>) -> Result<Response<MockBody>, Infallible> {
        let response = match request.uri().path() {
            "/containers/json" => {
                let list = self.containers.lock().unwrap().to_string();
                Response::new(Full::new(Bytes::from(list)).boxed())
            }
            "/events" => {
                let events = BroadcastStream::new(self.events.subscribe()).map(|_| {
                    let event =
                        Bytes::from_static(b"{\"Type\":\"container\",\"Action\":\"start\"}\n");
                    Ok::<_, Infallible>(Frame::data(event))
                });
                Response::new(StreamBody::new(events).boxed())
            }
            _ => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from_static(b"{\"message\":\"page not found\"}")).boxed())
                .unwrap(),
        };
        Ok(response)
    }
}

fn container(name: &str, port_name: &str, upstream_port: u16) -> Value {
    json!({
        "Id": format!("{name}-0123456789"),
        "Names": [format!("/{name}")],
        "Labels": {
            "r3v3rs3.enable": "true",
            "r3v3rs3.http.app.ports": port_name,
            "r3v3rs3.http.app.port": upstream_port.to_string(),
        },
        "Status": "Up 1 second",
        "HostConfig": {"NetworkMode": "bridge"},
        "NetworkSettings": {"Networks": {"bridge": {"IPAddress": "127.0.0.1"}}},
    })
}

#[tokio::test]
async fn docker_labels_define_proxies_that_follow_container_events() -> anyhow::Result<()> {
    let upstream_port = serve_docker_upstream().await?;
    let proxy_port = alloc_tcp_port().await?;
    let mut port = http_port_entry("test", &proxy_port);
    port.port.name = "web".into();

    let socket = std::env::temp_dir().join(format!("r3v3rs3-docker-{}.sock", std::process::id()));
    let mock = MockDocker {
        containers: Arc::new(Mutex::new(json!([container(
            "web-1",
            "web",
            upstream_port
        )]))),
        events: broadcast::channel(16).0,
    };
    mock.serve(&socket).await?;

    let mut config = AppConfig::default();
    config.discovery.docker.enabled = true;
    config.discovery.docker.endpoint = "docker.sock".into();
    let storage = TestStorage::builder().ports(vec![port]).build();
    let url = proxy_port.http_url("/").to_string();
    let endpoint = format!("unix://{}", socket.display());
    with_server(storage, |mut channels| async move {
        check_endpoint_then_running(&mut channels, &mut config, endpoint, &url).await?;
        check_unknown_port_issue(&mut channels, &mock, upstream_port).await?;
        check_stop_and_disable(&mut channels, &mock, &url, config).await
    })
    .await?;
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

/// Serves the upstream that answers "docker" and returns its port.
async fn serve_docker_upstream() -> anyhow::Result<u16> {
    let upstream =
        serve_http_upstream(Router::new().route("/", get(|| async { "docker" }))).await?;
    upstream
        .port()
        .ok_or_else(|| anyhow::anyhow!("the upstream URL has no port"))
}

/// Checks that a relative endpoint is rejected, then that the Unix socket endpoint starts the
/// provider and its proxy serves traffic.
async fn check_endpoint_then_running(
    channels: &mut ServerChannels,
    config: &mut AppConfig,
    endpoint: String,
    url: &str,
) -> anyhow::Result<()> {
    let rejected = call(
        channels,
        SetConfig {
            config: config.clone(),
        },
    )
    .await?;
    assert!(matches!(
        rejected,
        Err(Error::InvalidDiscoveryConfig { .. })
    ));

    config.discovery.docker.endpoint = endpoint;
    call(
        channels,
        SetConfig {
            config: config.clone(),
        },
    )
    .await??;
    assert_eq!(wait_for_status(url, 200).await?, "docker");
    wait_for_discovery(channels, |statuses| {
        statuses
            .iter()
            .any(|status| status.state == DiscoveryState::Running && status.proxies == 1)
    })
    .await?;
    Ok(())
}

/// Checks that a new container with an unknown port name becomes an issue.
async fn check_unknown_port_issue(
    channels: &mut ServerChannels,
    mock: &MockDocker,
    upstream_port: u16,
) -> anyhow::Result<()> {
    mock.set_containers(json!([
        container("web-1", "web", upstream_port),
        container("web-2", "missing", upstream_port),
    ]));
    let statuses = wait_for_discovery(channels, |statuses| {
        statuses.iter().any(|status| !status.issues.is_empty())
    })
    .await?;
    assert_eq!(statuses[0].issues[0].resource, "web-2");
    assert_eq!(
        statuses[0].issues[0].message,
        "http.app: port not found: missing"
    );
    Ok(())
}

/// Checks that a stopped container removes the proxy and that disabling the provider removes
/// its status.
async fn check_stop_and_disable(
    channels: &mut ServerChannels,
    mock: &MockDocker,
    url: &str,
    mut config: AppConfig,
) -> anyhow::Result<()> {
    // The container stops, so the proxy is removed.
    mock.set_containers(json!([]));
    wait_for_status(url, 502).await?;

    // Disabling the provider removes its status.
    config.discovery.docker.enabled = false;
    call(channels, SetConfig { config }).await??;
    wait_for_discovery(channels, <[DiscoveryStatus]>::is_empty).await?;
    Ok(())
}
