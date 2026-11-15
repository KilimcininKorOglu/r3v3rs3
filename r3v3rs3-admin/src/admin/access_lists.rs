use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::access_lists::{
    AddAccessList, DeleteAccessList, GetAccessLists, UpdateAccessList,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::{
    access_list::{AccessList, AccessListEntry},
    id::ShortId,
};

/// Lists the access lists without the password hashes and the token digests.
#[utoipa::path(
    get,
    path = "/",
    tag = "access_lists",
    operation_id = "list_access_lists",
    responses((status = 200, description = "The access lists.", body = Vec<AccessListEntry>), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<Vec<AccessListEntry>>>, AppError> {
    Ok(Json(state.call(&caller, GetAccessLists).await?))
}

/// Adds an access list.
#[utoipa::path(
    post,
    path = "/",
    tag = "access_lists",
    operation_id = "add_access_list",
    request_body = AccessList,
    responses((status = 200, description = "The access list is added."), ErrorResponses)
)]
pub async fn add(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(list): Json<AccessList>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(&caller, AddAccessList { list }).await?))
}

/// Replaces an access list. A user or a token without a new secret keeps its current secret.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "access_lists",
    operation_id = "update_access_list",
    params(("id" = ShortId, Path, description = "Access list id.")),
    request_body = AccessList,
    responses((status = 200, description = "The access list is updated."), NotFoundResponse, ErrorResponses)
)]
pub async fn put(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(list): Json<AccessList>,
) -> Result<Json<Box<()>>, AppError> {
    let entry = AccessListEntry { id, list };
    Ok(Json(state.call(&caller, UpdateAccessList { entry }).await?))
}

/// Deletes an access list.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "access_lists",
    operation_id = "delete_access_list",
    params(("id" = ShortId, Path, description = "Access list id.")),
    responses((status = 200, description = "The access list is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(&caller, DeleteAccessList { id }).await?))
}
