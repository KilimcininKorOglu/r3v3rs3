use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::dns_credential::{
    AddDnsCredential, DeleteDnsCredential, GetDnsCredential, GetDnsCredentialList,
    UpdateDnsCredential,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::{
    acme::{DnsCredentialInfo, DnsCredentialRequest},
    id::ShortId,
};

/// Lists the named DNS credentials. The answer holds no secret.
#[utoipa::path(
    get,
    path = "/",
    tag = "dns_credentials",
    operation_id = "list_dns_credentials",
    responses((status = 200, description = "The DNS credentials.", body = Vec<DnsCredentialInfo>), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<Vec<DnsCredentialInfo>>>, AppError> {
    Ok(Json(state.call(&caller, GetDnsCredentialList).await?))
}

/// Returns one named DNS credential without its secrets.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "dns_credentials",
    operation_id = "get_dns_credential",
    params(("id" = ShortId, Path, description = "DNS credential id.")),
    responses((status = 200, description = "The DNS credential.", body = DnsCredentialInfo), NotFoundResponse, ErrorResponses)
)]
pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<DnsCredentialInfo>>, AppError> {
    Ok(Json(state.call(&caller, GetDnsCredential { id }).await?))
}

/// Creates a named DNS credential.
#[utoipa::path(
    post,
    path = "/",
    tag = "dns_credentials",
    operation_id = "add_dns_credential",
    request_body = DnsCredentialRequest,
    responses((status = 200, description = "The DNS credential is created.", body = DnsCredentialInfo), ErrorResponses)
)]
pub async fn add(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<DnsCredentialRequest>,
) -> Result<Json<Box<DnsCredentialInfo>>, AppError> {
    Ok(Json(
        state.call(&caller, AddDnsCredential { request }).await?,
    ))
}

/// Replaces the name and optionally the credentials of a named DNS credential.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "dns_credentials",
    operation_id = "update_dns_credential",
    params(("id" = ShortId, Path, description = "DNS credential id.")),
    request_body = DnsCredentialRequest,
    responses((status = 200, description = "The DNS credential is updated."), NotFoundResponse, ErrorResponses)
)]
pub async fn put(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<DnsCredentialRequest>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(
        state
            .call(&caller, UpdateDnsCredential { id, request })
            .await?,
    ))
}

/// Deletes a named DNS credential. A credential that an ACME entry references is not deleted.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "dns_credentials",
    operation_id = "delete_dns_credential",
    params(("id" = ShortId, Path, description = "DNS credential id.")),
    responses((status = 200, description = "The DNS credential is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(&caller, DeleteDnsCredential { id }).await?))
}
