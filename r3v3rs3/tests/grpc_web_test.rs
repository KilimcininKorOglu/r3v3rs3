use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use r3v3rs3_api::proxy::{HttpProxy, Route};

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route,
    serve_http_upstream, with_server,
};

#[tokio::test]
async fn test_grpc_web_cors_preflight() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let dummy_upstream = "http://127.0.0.1:9";

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", dummy_upstream, None)],
        grpc_web: true,
        ..Default::default()
    };

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("grpcpre", "http", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        let url = port.http_url("/helloworld.Greeter/SayHello");

        let res = client
            .request(reqwest::Method::OPTIONS, url)
            .header("Host", "localhost")
            .header("Origin", "http://localhost:3000")
            .header("Access-Control-Request-Method", "POST")
            .header(
                "Access-Control-Request-Headers",
                "content-type,x-grpc-web,x-user-agent",
            )
            .send()
            .await?;

        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            res.headers().get("access-control-allow-origin").unwrap(),
            "http://localhost:3000"
        );
        assert_eq!(
            res.headers().get("access-control-allow-methods").unwrap(),
            "POST, OPTIONS"
        );
        assert!(
            res.headers()
                .get("access-control-allow-headers")
                .unwrap()
                .to_str()?
                .contains("x-grpc-web")
        );
        assert!(
            res.headers()
                .get("access-control-expose-headers")
                .unwrap()
                .to_str()?
                .contains("grpc-status")
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_grpc_web_request_transpilation_and_response_framing() -> anyhow::Result<()> {
    let app = Router::new().route(
        "/helloworld.Greeter/SayHello",
        post(|headers: HeaderMap, body: Bytes| async move {
            // Verify upstream received transpiled application/grpc content-type
            let ct = headers.get("content-type").and_then(|v| v.to_str().ok());
            assert_eq!(ct, Some("application/grpc"));
            assert_eq!(
                headers.get("te").and_then(|v| v.to_str().ok()),
                Some("trailers")
            );

            let mut res_headers = HeaderMap::new();
            res_headers.insert("content-type", HeaderValue::from_static("application/grpc"));
            res_headers.insert("grpc-status", HeaderValue::from_static("0"));
            res_headers.insert("grpc-message", HeaderValue::from_static("OK"));

            (res_headers, body).into_response()
        }),
    );

    let upstream = serve_http_upstream(app).await?;
    let port = alloc_tcp_port().await?;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", upstream.as_str(), None)],
        grpc_web: true,
        ..Default::default()
    };

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("grpc-web", "http", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        let url = port.http_url("/helloworld.Greeter/SayHello");

        let request_payload = vec![0x00, 0x00, 0x00, 0x00, 0x05, b'h', b'e', b'l', b'l', b'o'];

        let res = client
            .post(url)
            .header("Host", "localhost")
            .header("Origin", "http://localhost:3000")
            .header("Content-Type", "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .body(request_payload)
            .send()
            .await?;

        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("content-type").unwrap(),
            "application/grpc-web+proto"
        );
        assert_eq!(
            res.headers().get("access-control-allow-origin").unwrap(),
            "http://localhost:3000"
        );
        assert!(
            res.headers()
                .get("access-control-expose-headers")
                .unwrap()
                .to_str()?
                .contains("grpc-status")
        );

        let body_bytes = res.bytes().await?;
        assert!(body_bytes.len() >= 10);
        // The response body should include the data and the trailer frame (0x80)
        assert!(body_bytes.contains(&0x80));
        let body_str = String::from_utf8_lossy(&body_bytes);
        assert!(body_str.contains("grpc-status: 0"));

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_grpc_web_route_override() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let dummy_upstream = "http://127.0.0.1:9";

    let mut route_enabled = http_route("/enabled", dummy_upstream, None);
    route_enabled.grpc_web = Some(true);

    let route_disabled = Route {
        path: "/disabled".into(),
        grpc_web: Some(false),
        ..http_route("/disabled", dummy_upstream, None)
    };

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![route_enabled, route_disabled],
        grpc_web: false, // proxy default is false
        ..Default::default()
    };

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("override", "http", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();

        // /enabled route should handle gRPC-Web CORS preflight
        let res = client
            .request(
                reqwest::Method::OPTIONS,
                port.http_url("/enabled/helloworld.Greeter/SayHello"),
            )
            .header("Host", "localhost")
            .header("Origin", "http://localhost:3000")
            .header("x-grpc-web", "1")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert!(res.headers().contains_key("access-control-allow-origin"));

        Ok(())
    })
    .await
}
