//! Admin API endpoints for gRPC reflection and interactive method invocation.

use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::proxies::GetProxy;
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::{
    error::Error,
    grpc::{GrpcInvokeRequest, GrpcInvokeResponse, GrpcReflectRequest, GrpcReflectResponse},
    id::ShortId,
    proxy::{Proxy, ProxyKind},
};

fn resolve_proxy_target(proxy: &Proxy) -> Option<String> {
    match &proxy.kind {
        ProxyKind::Http(http) => http
            .routes
            .iter()
            .flat_map(|r| &r.servers)
            .map(|s| s.url.to_string())
            .next(),
        _ => None,
    }
}

/// Discovers gRPC services and methods using Server Reflection on a proxy's upstream.
#[utoipa::path(
    post,
    path = "/{id}/grpc/reflect",
    tag = "proxies",
    operation_id = "reflect_proxy_grpc",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    request_body(content = Option<GrpcReflectRequest>),
    responses(
        (status = 200, description = "Discovered gRPC services and schemas.", body = GrpcReflectResponse),
        NotFoundResponse,
        ErrorResponses
    )
)]
pub async fn reflect_proxy(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(body): Json<Option<GrpcReflectRequest>>,
) -> Result<Json<GrpcReflectResponse>, AppError> {
    let entry = state.call(&caller, GetProxy { id }).await?;
    let target = body
        .as_ref()
        .and_then(|b| b.target_url.clone())
        .or_else(|| resolve_proxy_target(&entry.proxy))
        .ok_or_else(|| Error::GrpcError {
            reason: "no upstream servers configured for gRPC reflection".to_string(),
        })?;

    let res = crate::grpc::discover_grpc_services(&target)
        .await
        .map_err(|err| Error::GrpcError {
            reason: err.to_string(),
        })?;
    Ok(Json(res))
}

/// Invokes a gRPC unary method on a proxy's upstream.
#[utoipa::path(
    post,
    path = "/{id}/grpc/invoke",
    tag = "proxies",
    operation_id = "invoke_proxy_grpc",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    request_body = GrpcInvokeRequest,
    responses(
        (status = 200, description = "gRPC call response and metadata.", body = GrpcInvokeResponse),
        NotFoundResponse,
        ErrorResponses
    )
)]
pub async fn invoke_proxy(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(mut req): Json<GrpcInvokeRequest>,
) -> Result<Json<GrpcInvokeResponse>, AppError> {
    let entry = state.call(&caller, GetProxy { id }).await?;
    let target = req
        .target_url
        .clone()
        .or_else(|| resolve_proxy_target(&entry.proxy))
        .ok_or_else(|| Error::GrpcError {
            reason: "no upstream servers configured for gRPC invoke".to_string(),
        })?;

    req.target_url = Some(target.clone());
    let res = crate::grpc::execute_grpc_call(&target, &req)
        .await
        .map_err(|err| Error::GrpcError {
            reason: err.to_string(),
        })?;
    Ok(Json(res))
}

/// Discovers gRPC services and methods directly from a target URL.
#[utoipa::path(
    post,
    path = "/reflect",
    tag = "grpc",
    operation_id = "reflect_grpc",
    request_body = GrpcReflectRequest,
    responses(
        (status = 200, description = "Discovered gRPC services and schemas.", body = GrpcReflectResponse),
        ErrorResponses
    )
)]
pub async fn reflect_standalone(
    State(_state): State<AppState>,
    Extension(_caller): Extension<Caller>,
    Json(req): Json<GrpcReflectRequest>,
) -> Result<Json<GrpcReflectResponse>, AppError> {
    let target = req.target_url.ok_or_else(|| Error::GrpcError {
        reason: "target_url is required".to_string(),
    })?;

    let res = crate::grpc::discover_grpc_services(&target)
        .await
        .map_err(|err| Error::GrpcError {
            reason: err.to_string(),
        })?;
    Ok(Json(res))
}

/// Invokes a gRPC unary method directly on a target URL.
#[utoipa::path(
    post,
    path = "/invoke",
    tag = "grpc",
    operation_id = "invoke_grpc",
    request_body = GrpcInvokeRequest,
    responses(
        (status = 200, description = "gRPC call response and metadata.", body = GrpcInvokeResponse),
        ErrorResponses
    )
)]
pub async fn invoke_standalone(
    State(_state): State<AppState>,
    Extension(_caller): Extension<Caller>,
    Json(req): Json<GrpcInvokeRequest>,
) -> Result<Json<GrpcInvokeResponse>, AppError> {
    let target = req.target_url.as_deref().ok_or_else(|| Error::GrpcError {
        reason: "target_url is required".to_string(),
    })?;

    let res = crate::grpc::execute_grpc_call(target, &req)
        .await
        .map_err(|err| Error::GrpcError {
            reason: err.to_string(),
        })?;
    Ok(Json(res))
}
