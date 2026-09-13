use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::server::rpc::proxies::{
    AddProxy, DeleteProxy, GetProxy, GetProxyList, GetProxyStatus, PurgeProxyCache, UpdateProxy,
};
use axum::{
    extract::{Path, State},
    Json,
};
use r3v3rs3_api::{
    id::ShortId,
    proxy::{Proxy, ProxyEntry, ProxyStatus},
};

/// Lists the proxies.
#[utoipa::path(
    get,
    path = "/",
    tag = "proxies",
    operation_id = "list_proxies",
    responses((status = 200, description = "The proxies.", body = Vec<ProxyEntry>), ErrorResponses)
)]
pub async fn list(State(state): State<AppState>) -> Result<Json<Box<Vec<ProxyEntry>>>, AppError> {
    Ok(Json(state.call(GetProxyList).await?))
}

/// Returns one proxy.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "proxies",
    operation_id = "get_proxy",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    responses((status = 200, description = "The proxy.", body = ProxyEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<ProxyEntry>>, AppError> {
    Ok(Json(state.call(GetProxy { id }).await?))
}

/// Returns the state of a proxy.
#[utoipa::path(
    get,
    path = "/{id}/status",
    tag = "proxies",
    operation_id = "get_proxy_status",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    responses((status = 200, description = "The proxy status.", body = ProxyStatus), NotFoundResponse, ErrorResponses)
)]
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<ProxyStatus>>, AppError> {
    Ok(Json(state.call(GetProxyStatus { id }).await?))
}

/// Deletes a proxy.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "proxies",
    operation_id = "delete_proxy",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    responses((status = 200, description = "The proxy is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(DeleteProxy { id }).await?))
}

/// Removes the stored responses from the HTTP cache of a proxy.
#[utoipa::path(
    delete,
    path = "/{id}/cache",
    tag = "proxies",
    operation_id = "purge_proxy_cache",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    responses((status = 200, description = "The cache is empty."), NotFoundResponse, ErrorResponses)
)]
pub async fn purge_cache(
    State(state): State<AppState>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(PurgeProxyCache { id }).await?))
}

/// Adds a proxy.
#[utoipa::path(
    post,
    path = "/",
    tag = "proxies",
    operation_id = "add_proxy",
    request_body = Proxy,
    responses((status = 200, description = "The proxy is added."), ErrorResponses)
)]
pub async fn add(
    State(state): State<AppState>,
    Json(entry): Json<Proxy>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(AddProxy { entry }).await?))
}

/// Replaces the config of a proxy.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "proxies",
    operation_id = "update_proxy",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    request_body = Proxy,
    responses((status = 200, description = "The proxy is updated."), NotFoundResponse, ErrorResponses)
)]
pub async fn put(
    State(state): State<AppState>,
    Path(id): Path<ShortId>,
    Json(entry): Json<Proxy>,
) -> Result<Json<Box<()>>, AppError> {
    let entry = (id, entry).into();
    Ok(Json(state.call(UpdateProxy { entry }).await?))
}
