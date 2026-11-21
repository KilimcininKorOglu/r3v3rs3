//! gRPC Server Reflection discovery (`r3v3rs3::grpc::discover_grpc_services`), checked against
//! the gRPC Server Reflection protocol (grpc/proposal A15): a server that cannot be reached, that
//! answers with an HTTP error, or that implements neither reflection version must be reported as
//! an error, never as a server with no services.

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::IntoResponse;
use axum::routing::post;
use r3v3rs3::grpc::discover_grpc_services;

use crate::common::serve_http_upstream;

/// The reflection client builds a reqwest client, which needs a rustls crypto provider.
async fn discover(url: &str) -> anyhow::Result<r3v3rs3_api::grpc::GrpcReflectResponse> {
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
    discover_grpc_services(url).await
}

const V1_PATH: &str = "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo";
const V1ALPHA_PATH: &str = "/grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo";

fn grpc_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x00];
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Prefixes `body` with the protobuf tag of a length-delimited `field`.
fn length_delimited(field: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![(field << 3) | 2, body.len() as u8];
    out.extend_from_slice(body);
    out
}

/// A `ServerReflectionResponse` whose `list_services_response` (field 5) names one service.
fn list_services_answer(service: &str) -> Vec<u8> {
    let service_response = length_delimited(1, service.as_bytes());
    let list = length_delimited(1, &service_response);
    grpc_frame(&length_delimited(5, &list))
}

fn grpc_headers(status: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/grpc"));
    headers.insert("grpc-status", HeaderValue::from_static(status));
    headers
}

/// A handler that answers every call with the gRPC status `UNIMPLEMENTED` (12).
fn unimplemented() -> axum::routing::MethodRouter {
    post(|| async { (grpc_headers("12"), Bytes::new()).into_response() })
}

#[tokio::test]
async fn an_unreachable_server_is_an_error() {
    // Port 1 is closed on the test host, so the connection is refused.
    let result = discover("http://127.0.0.1:1").await;
    assert!(result.is_err(), "{result:?}");
}

#[tokio::test]
async fn an_http_error_is_an_error() -> anyhow::Result<()> {
    // No route at all: every reflection call gets HTTP 404.
    let upstream = serve_http_upstream(Router::new()).await?;
    let result = discover(upstream.as_str()).await;
    assert!(result.is_err(), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn a_server_without_reflection_is_an_error() -> anyhow::Result<()> {
    let app = Router::new()
        .route(V1_PATH, unimplemented())
        .route(V1ALPHA_PATH, unimplemented());
    let upstream = serve_http_upstream(app).await?;
    let err = discover(upstream.as_str())
        .await
        .expect_err("a server without reflection listed no services as a success");
    assert!(err.to_string().contains("reflection"), "{err}");
    Ok(())
}

#[tokio::test]
async fn a_failing_reflection_call_is_an_error() -> anyhow::Result<()> {
    // grpc-status 14 (UNAVAILABLE) is a failure, not "reflection is not implemented".
    let app = Router::new().route(
        V1_PATH,
        post(|| async { (grpc_headers("14"), Bytes::new()).into_response() }),
    );
    let upstream = serve_http_upstream(app).await?;
    let result = discover(upstream.as_str()).await;
    assert!(result.is_err(), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn the_older_reflection_version_is_used_when_v1_is_missing() -> anyhow::Result<()> {
    let app = Router::new().route(V1_PATH, unimplemented()).route(
        V1ALPHA_PATH,
        post(|body: Bytes| async move {
            // `list_services` (field 7) asks for the list; any other request gets UNIMPLEMENTED.
            if body.get(5) == Some(&((7 << 3) | 2)) {
                (grpc_headers("0"), list_services_answer("demo.Echo")).into_response()
            } else {
                (grpc_headers("12"), Bytes::new()).into_response()
            }
        }),
    );
    let upstream = serve_http_upstream(app).await?;
    let found = discover(upstream.as_str()).await?;
    assert_eq!(found.reflection_version, "v1alpha");
    let names: Vec<_> = found.services.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["demo.Echo"]);
    Ok(())
}
