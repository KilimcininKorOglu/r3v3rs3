use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::acme::{AddAcme, DeleteAcme, GetAcme, GetAcmeList, UpdateAcme};
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::{
    acme::{AcmeConfig, AcmeInfo, AcmeRequest},
    id::ShortId,
};

/// Lists the ACME requests.
#[utoipa::path(
    get,
    path = "/",
    tag = "acme",
    operation_id = "list_acme",
    responses((status = 200, description = "The ACME requests.", body = Vec<AcmeInfo>), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<Vec<AcmeInfo>>>, AppError> {
    Ok(Json(state.call(&caller, GetAcmeList).await?))
}

/// Returns one ACME request.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "acme",
    operation_id = "get_acme",
    params(("id" = ShortId, Path, description = "ACME request id.")),
    responses((status = 200, description = "The ACME request.", body = AcmeInfo), NotFoundResponse, ErrorResponses)
)]
pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<AcmeInfo>>, AppError> {
    Ok(Json(state.call(&caller, GetAcme { id }).await?))
}

/// Creates an ACME account and adds the request.
#[utoipa::path(
    post,
    path = "/",
    tag = "acme",
    operation_id = "add_acme",
    request_body = AcmeRequest,
    responses((status = 200, description = "The ACME request is added."), ErrorResponses)
)]
pub async fn add(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<AcmeRequest>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(&caller, AddAcme { request }).await?))
}

/// Replaces the config of an ACME request.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "acme",
    operation_id = "update_acme",
    params(("id" = ShortId, Path, description = "ACME request id.")),
    request_body = AcmeConfig,
    responses((status = 200, description = "The ACME request is updated."), NotFoundResponse, ErrorResponses)
)]
pub async fn put(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(config): Json<AcmeConfig>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(&caller, UpdateAcme { id, config }).await?))
}

/// Deletes an ACME request.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "acme",
    operation_id = "delete_acme",
    params(("id" = ShortId, Path, description = "ACME request id.")),
    responses((status = 200, description = "The ACME request is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(&caller, DeleteAcme { id }).await?))
}
