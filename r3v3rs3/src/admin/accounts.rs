use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::accounts::{AddAccount, DeleteAccount, GetAccountList, UpdateAccount};
use axum::{
    extract::{Path, State},
    Extension, Json,
};
use r3v3rs3_api::auth::{AccountCreated, AccountInfo, AccountUpdate, NewAccount};

/// Lists the accounts without their secrets.
#[utoipa::path(
    get,
    path = "/",
    tag = "accounts",
    operation_id = "list_accounts",
    responses((status = 200, description = "The accounts.", body = Vec<AccountInfo>), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<Vec<AccountInfo>>>, AppError> {
    Ok(Json(state.call(&caller, GetAccountList).await?))
}

/// Adds an account. The response holds the TOTP secret of the account, and no later response
/// shows it again.
#[utoipa::path(
    post,
    path = "/",
    tag = "accounts",
    operation_id = "add_account",
    request_body = NewAccount,
    responses(
        (status = 200, description = "The account is added.", body = AccountCreated),
        (status = 409, description = "An account with this username already exists."),
        ErrorResponses
    )
)]
pub async fn add(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(account): Json<NewAccount>,
) -> Result<Json<Box<AccountCreated>>, AppError> {
    Ok(Json(state.call(&caller, AddAccount { account }).await?))
}

/// Changes the role, the proxy list or the password of an account. A new password ends the
/// sessions of the account.
#[utoipa::path(
    put,
    path = "/{username}",
    tag = "accounts",
    operation_id = "update_account",
    params(("username" = String, Path, description = "Username.")),
    request_body = AccountUpdate,
    responses((status = 200, description = "The account is updated."), NotFoundResponse, ErrorResponses)
)]
pub async fn put(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(username): Path<String>,
    Json(update): Json<AccountUpdate>,
) -> Result<Json<Box<()>>, AppError> {
    let method = UpdateAccount {
        actor: caller.username.clone(),
        username,
        update,
    };
    Ok(Json(state.call(&caller, method).await?))
}

/// Deletes an account and ends its sessions.
#[utoipa::path(
    delete,
    path = "/{username}",
    tag = "accounts",
    operation_id = "delete_account",
    params(("username" = String, Path, description = "Username.")),
    responses((status = 200, description = "The account is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(username): Path<String>,
) -> Result<Json<Box<()>>, AppError> {
    let method = DeleteAccount {
        actor: caller.username.clone(),
        username,
    };
    Ok(Json(state.call(&caller, method).await?))
}
