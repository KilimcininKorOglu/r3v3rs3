use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::error_page_themes::{
    AddErrorPageTheme, DeleteErrorPageTheme, GetErrorPageTheme, GetErrorPageThemes,
    ResetErrorPageThemes, UpdateErrorPageTheme,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::error_page_theme::ErrorPageTheme;
use r3v3rs3_api::id::ShortId;

/// Lists all error page themes.
#[utoipa::path(
    get,
    path = "/",
    tag = "error_page_themes",
    operation_id = "list_error_page_themes",
    responses(
        (status = 200, description = "The error page themes.", body = Vec<ErrorPageTheme>),
        ErrorResponses
    )
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<Vec<ErrorPageTheme>>>, AppError> {
    Ok(Json(state.call(&caller, GetErrorPageThemes).await?))
}

/// Adds a new error page theme.
#[utoipa::path(
    post,
    path = "/",
    tag = "error_page_themes",
    operation_id = "add_error_page_theme",
    request_body = ErrorPageTheme,
    responses(
        (status = 200, description = "The error page theme was created.", body = ShortId),
        ErrorResponses
    )
)]
pub async fn add(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(theme): Json<ErrorPageTheme>,
) -> Result<Json<Box<ShortId>>, AppError> {
    Ok(Json(
        state.call(&caller, AddErrorPageTheme { theme }).await?,
    ))
}

/// Gets an error page theme by id.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "error_page_themes",
    operation_id = "get_error_page_theme",
    params(("id" = ShortId, Path, description = "Error page theme id.")),
    responses(
        (status = 200, description = "The error page theme.", body = ErrorPageTheme),
        NotFoundResponse,
        ErrorResponses
    )
)]
pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<ErrorPageTheme>>, AppError> {
    Ok(Json(state.call(&caller, GetErrorPageTheme { id }).await?))
}

/// Updates an existing error page theme.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "error_page_themes",
    operation_id = "update_error_page_theme",
    params(("id" = ShortId, Path, description = "Error page theme id.")),
    request_body = ErrorPageTheme,
    responses(
        (status = 200, description = "The error page theme was updated."),
        NotFoundResponse,
        ErrorResponses
    )
)]
pub async fn put(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(mut theme): Json<ErrorPageTheme>,
) -> Result<Json<Box<()>>, AppError> {
    theme.id = id;
    Ok(Json(
        state.call(&caller, UpdateErrorPageTheme { theme }).await?,
    ))
}

/// Deletes an error page theme by id.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "error_page_themes",
    operation_id = "delete_error_page_theme",
    params(("id" = ShortId, Path, description = "Error page theme id.")),
    responses(
        (status = 200, description = "The error page theme was deleted."),
        NotFoundResponse,
        ErrorResponses
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(
        state.call(&caller, DeleteErrorPageTheme { id }).await?,
    ))
}

/// Resets error page themes to default built-in templates.
#[utoipa::path(
    post,
    path = "/reset-defaults",
    tag = "error_page_themes",
    operation_id = "reset_error_page_themes",
    responses(
        (status = 200, description = "The default error page themes were restored.", body = Vec<ErrorPageTheme>),
        ErrorResponses
    )
)]
pub async fn reset_defaults(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<Vec<ErrorPageTheme>>>, AppError> {
    Ok(Json(state.call(&caller, ResetErrorPageThemes).await?))
}
