use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use r3v3rs3::admin::start_admin;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::grpc::health::check_grpc_health;
use r3v3rs3::log::DatabaseLayer;
use r3v3rs3_api::grpc::{GrpcHealthCheckRequest, GrpcHealthCheckResponse, GrpcServingStatus};
use r3v3rs3_api::proxy::{HttpProxy, Server};
use r3v3rs3_api::upstream::HealthCheck;
use std::collections::HashMap;
use std::time::Duration;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{
    TestStorage, admin_session_cookie, alloc_tcp_port, http_port_entry, http_proxy_entry,
    http_route, serve_http_upstream, wait_for_listener, with_server,
};

fn init_crypto_provider() {
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
}

fn make_grpc_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(0x00);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

#[tokio::test]
async fn test_grpc_health_check_statuses() -> anyhow::Result<()> {
    init_crypto_provider();
    // Upstream 1: Returns SERVING (1)
    let app_serving = Router::new().route(
        "/grpc.health.v1.Health/Check",
        post(|headers: HeaderMap, _body: Bytes| async move {
            assert_eq!(
                headers.get("content-type").and_then(|v| v.to_str().ok()),
                Some("application/grpc")
            );
            let mut res_headers = HeaderMap::new();
            res_headers.insert("content-type", HeaderValue::from_static("application/grpc"));
            res_headers.insert("grpc-status", HeaderValue::from_static("0"));
            let body = make_grpc_frame(&[0x08, 0x01]);
            (res_headers, body).into_response()
        }),
    );

    // Upstream 2: Returns NOT_SERVING (2)
    let app_not_serving = Router::new().route(
        "/grpc.health.v1.Health/Check",
        post(|_headers: HeaderMap, _body: Bytes| async move {
            let mut res_headers = HeaderMap::new();
            res_headers.insert("content-type", HeaderValue::from_static("application/grpc"));
            res_headers.insert("grpc-status", HeaderValue::from_static("0"));
            let body = make_grpc_frame(&[0x08, 0x02]);
            (res_headers, body).into_response()
        }),
    );

    // Upstream 3: Returns gRPC UNIMPLEMENTED (12)
    let app_unimplemented = Router::new().route(
        "/grpc.health.v1.Health/Check",
        post(|_headers: HeaderMap, _body: Bytes| async move {
            let mut res_headers = HeaderMap::new();
            res_headers.insert("content-type", HeaderValue::from_static("application/grpc"));
            res_headers.insert("grpc-status", HeaderValue::from_static("12"));
            res_headers.insert(
                "grpc-message",
                HeaderValue::from_static("service not found"),
            );
            (res_headers, Bytes::new()).into_response()
        }),
    );

    let upstream1 = serve_http_upstream(app_serving).await?;
    let upstream2 = serve_http_upstream(app_not_serving).await?;
    let upstream3 = serve_http_upstream(app_unimplemented).await?;

    let client = reqwest::Client::builder().build()?;

    // 1. Check serving upstream
    let res1 = check_grpc_health(&client, upstream1.as_str(), "")
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    assert!(res1.healthy);
    assert_eq!(res1.status, GrpcServingStatus::Serving);
    assert_eq!(res1.grpc_status, 0);

    // 2. Check not serving upstream
    let res2 = check_grpc_health(&client, upstream2.as_str(), "my.service")
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    assert!(!res2.healthy);
    assert_eq!(res2.status, GrpcServingStatus::NotServing);
    assert_eq!(res2.grpc_status, 0);

    // 3. Check unimplemented upstream
    let res3 = check_grpc_health(&client, upstream3.as_str(), "")
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    assert!(!res3.healthy);
    assert_eq!(res3.status, GrpcServingStatus::Unknown);
    assert_eq!(res3.grpc_status, 12);
    assert_eq!(res3.grpc_status_name, "UNIMPLEMENTED");
    assert_eq!(res3.grpc_message.as_deref(), Some("service not found"));

    Ok(())
}

#[tokio::test]
async fn test_admin_api_grpc_health_endpoints() -> anyhow::Result<()> {
    init_crypto_provider();
    let dir = std::env::temp_dir().join(format!("r3v3rs3-grpc-health-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;

    let app_serving = Router::new().route(
        "/grpc.health.v1.Health/Check",
        post(|_headers: HeaderMap, _body: Bytes| async move {
            let mut res_headers = HeaderMap::new();
            res_headers.insert("content-type", HeaderValue::from_static("application/grpc"));
            res_headers.insert("grpc-status", HeaderValue::from_static("0"));
            let body = make_grpc_frame(&[0x08, 0x01]);
            (res_headers, body).into_response()
        }),
    );

    let upstream = serve_http_upstream(app_serving).await?;
    let proxy_port = alloc_tcp_port().await?;
    let admin_port = alloc_tcp_port().await?;
    let admin_addr = admin_port.socket_addr();

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", upstream.as_str(), None)],
        health_check: HealthCheck {
            grpc_service: Some("helloworld.Greeter".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let storage = TestStorage::builder()
        .accounts(HashMap::from([("admin".to_string(), "secret".to_string())]))
        .ports(vec![http_port_entry("http", &proxy_port)])
        .proxies(vec![http_proxy_entry("grpcprox", "http", proxy)])
        .build();

    let app_info = new_appinfo(&dir, &dir);
    with_server(storage, |channels| async move {
        tokio::spawn(start_admin(
            app_info,
            admin_addr,
            channels.command,
            channels.callback,
            channels.event.clone(),
            channels.accounts.clone(),
        ));
        wait_for_listener(admin_addr).await?;

        let cookie = admin_session_cookie(admin_addr).await?;
        let client = reqwest::Client::new();
        let base_url = format!("http://{admin_addr}");

        // Test POST /api/grpc/health (standalone)
        let standalone_req = GrpcHealthCheckRequest {
            target_url: Some(upstream.to_string()),
            service: Some("helloworld.Greeter".to_string()),
        };
        let res = client
            .post(format!("{base_url}/api/grpc/health"))
            .header("Cookie", &cookie)
            .json(&standalone_req)
            .send()
            .await?;

        assert_eq!(res.status(), StatusCode::OK);
        let check_res: GrpcHealthCheckResponse = res.json().await?;
        assert!(check_res.healthy);
        assert_eq!(check_res.status, GrpcServingStatus::Serving);

        // Test POST /api/proxies/{id}/grpc/health (proxy level)
        let res_proxy = client
            .post(format!("{base_url}/api/proxies/grpcprox/grpc/health"))
            .header("Cookie", &cookie)
            .json(&serde_json::json!({}))
            .send()
            .await?;

        assert_eq!(res_proxy.status(), StatusCode::OK);
        let proxy_check_res: GrpcHealthCheckResponse = res_proxy.json().await?;
        assert!(proxy_check_res.healthy);
        assert_eq!(proxy_check_res.status, GrpcServingStatus::Serving);

        Ok(())
    })
    .await?;

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn test_proxy_grpc_active_health_check_failover() -> anyhow::Result<()> {
    init_crypto_provider();

    // Server 1 (first in list): Returns NOT_SERVING (2)
    let app_sick = Router::new()
        .route(
            "/grpc.health.v1.Health/Check",
            post(|_headers: HeaderMap, _body: Bytes| async move {
                let mut res_headers = HeaderMap::new();
                res_headers.insert("content-type", HeaderValue::from_static("application/grpc"));
                res_headers.insert("grpc-status", HeaderValue::from_static("0"));
                let body = make_grpc_frame(&[0x08, 0x02]);
                (res_headers, body).into_response()
            }),
        )
        .route("/", axum::routing::get(|| async { "sick" }));

    // Server 2: Returns SERVING (1)
    let app_live = Router::new()
        .route(
            "/grpc.health.v1.Health/Check",
            post(|_headers: HeaderMap, _body: Bytes| async move {
                let mut res_headers = HeaderMap::new();
                res_headers.insert("content-type", HeaderValue::from_static("application/grpc"));
                res_headers.insert("grpc-status", HeaderValue::from_static("0"));
                let body = make_grpc_frame(&[0x08, 0x01]);
                (res_headers, body).into_response()
            }),
        )
        .route("/", axum::routing::get(|| async { "live" }));

    let upstream_sick = serve_http_upstream(app_sick).await?;
    let upstream_live = serve_http_upstream(app_live).await?;
    let proxy_port = alloc_tcp_port().await?;

    use r3v3rs3_api::upstream::LoadBalancing;

    let servers = vec![
        Server::new(upstream_sick.as_str().parse()?),
        Server::new(upstream_live.as_str().parse()?),
    ];

    let mut route = http_route("/", upstream_sick.as_str(), None);
    route.servers = servers;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![route],
        load_balancing: LoadBalancing::First,
        health_check: HealthCheck {
            max_fails: 0,
            fail_timeout: Duration::from_secs(30),
            interval: Duration::from_millis(150),
            timeout: Duration::from_secs(1),
            grpc_service: Some("".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let storage = TestStorage::builder()
        .ports(vec![http_port_entry("http", &proxy_port)])
        .proxies(vec![http_proxy_entry("grpc-fo", "http", proxy)])
        .build();

    with_server(storage, |_channels| async move {
        let client = reqwest::Client::new();
        let url = proxy_port.http_url("/");

        let mut body = String::new();
        for _ in 0..50 {
            let req = async {
                let res = client
                    .get(url.clone())
                    .header("Host", "localhost")
                    .send()
                    .await
                    .ok()?;
                res.text().await.ok()
            };
            if let Some(text) = req.await {
                body = text;
                if body == "live" {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        assert_eq!(body, "live");
        Ok(())
    })
    .await
}
