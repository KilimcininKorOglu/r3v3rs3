//! The databases of the deployment platform. A database is an app with a `database` engine:
//! an image, one volume, a fixed port on `127.0.0.1` and no domains, so it gets no proxy. The
//! handlers reuse the app layer and show the generated password once.

use super::openapi::{ErrorResponses, NotFoundResponse};
use super::platform::platform_error;
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::clock::unix_ms;
use crate::platform::allocate_host_port;
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::{
    audit::AuditAction,
    container::{VolumeMount, VolumeName},
    error::Error,
    id::ShortId,
    platform::{
        AppEntry, AppRequest, AppSource, AppSpec, DatabaseCreated, DatabaseRequest,
        DeploymentTrigger, EngineInfo, LOCAL_TARGET,
    },
};
use std::sync::Arc;

/// The engines that the Databases page offers.
#[utoipa::path(
    get,
    path = "/engines",
    tag = "platform",
    operation_id = "list_database_engines",
    responses((status = 200, description = "The engines.", body = Vec<EngineInfo>), ErrorResponses)
)]
pub async fn engines(
    State(_state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<EngineInfo>>, AppError> {
    caller.authorize(Permission::Read)?;
    Ok(Json(r3v3rs3_api::platform::engines()))
}

/// Downloads the engine catalog from the repository and applies it.
#[utoipa::path(
    post,
    path = "/engines/refresh",
    tag = "platform",
    operation_id = "refresh_database_engines",
    responses((status = 200, description = "The engines.", body = Vec<EngineInfo>), ErrorResponses)
)]
pub async fn refresh_engines(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<EngineInfo>>, AppError> {
    caller.authorize(Permission::Edit)?;
    let catalog = crate::engines::fetch::fetch_catalog()
        .await
        .map_err(|err| {
            tracing::error!(%err, "failed to refresh the engine catalog");
            Error::FailedToRefreshEngines
        })?;
    state
        .sender
        .send(crate::command::ServerCommand::SetEngines { catalog })
        .await
        .map_err(|_| Error::FailedToInvokeRpc)?;
    let record = AuditRecord::new(AuditAction::RefreshEngines);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(r3v3rs3_api::platform::engines()))
}

/// Lists the database apps.
#[utoipa::path(
    get,
    path = "/",
    tag = "platform",
    operation_id = "list_databases",
    responses((status = 200, description = "The databases.", body = Vec<AppEntry>), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<AppEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let databases = platform
        .apps()
        .await
        .map_err(platform_error)?
        .into_iter()
        .filter(|app| app.spec.database.is_some())
        .collect();
    Ok(Json(databases))
}

/// Creates a database: an image app with a volume, a fixed host port and a generated
/// password, deployed at once. The password is in the response only.
#[utoipa::path(
    post,
    path = "/",
    tag = "platform",
    operation_id = "create_database",
    request_body = DatabaseRequest,
    responses((status = 200, description = "The new database.", body = DatabaseCreated), ErrorResponses)
)]
pub async fn create(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<DatabaseRequest>,
) -> Result<Json<DatabaseCreated>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let Some(info) = r3v3rs3_api::platform::engine_info(&request.engine) else {
        return Err(Error::InvalidContainerSpec {
            reason: format!(
                "the engine {} is not in the database catalog",
                request.engine
            ),
        }
        .into());
    };
    if !info.versions.iter().any(|tag| tag == &request.version) {
        return Err(Error::InvalidContainerSpec {
            reason: format!(
                "{} runs one of {}",
                request.engine,
                info.versions.join(", ")
            ),
        }
        .into());
    }
    let host_port = match request.host_port {
        Some(port) => port,
        None => allocate_host_port().map_err(platform_error)?,
    };
    let app_request = app_request(&request, &info, host_port).map_err(into_app_error)?;
    let app = platform
        .add_app(app_request, unix_ms())
        .await
        .map_err(platform_error)?;
    let password = set_password(&platform, &app, &info).await?;
    if let Err(err) = platform
        .deploy(app.id, &caller.username, DeploymentTrigger::Manual)
        .await
    {
        // The app stays for its log, like a failed deployment of the Apps page.
        return Err(platform_error(err));
    }
    let record = AuditRecord::new(AuditAction::CreateDatabase)
        .id(app.id)
        .summary(format!("{} {}", app.name, request.engine.clone()));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(DatabaseCreated {
        app,
        password,
        host_port,
        user: info.user.clone(),
        database: info.default_db.clone(),
    }))
}

/// Deletes a database app. Its volume stays.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "platform",
    operation_id = "delete_database",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The database is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<()>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let app = platform.delete_app(id).await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DeleteDatabase)
        .id(app.id)
        .summary(app.name.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(()))
}

/// The image app that carries the database: its catalog port, one data volume and the fixed
/// host port.
fn app_request(
    request: &DatabaseRequest,
    info: &EngineInfo,
    host_port: u16,
) -> Result<AppRequest, Error> {
    Ok(AppRequest {
        name: request.name.clone(),
        target: LOCAL_TARGET.parse()?,
        spec: AppSpec {
            source: AppSource::Image {
                image: request.version.parse()?,
            },
            port: info.container_port,
            domains: Vec::new(),
            ports: Vec::new(),
            health_check_path: None,
            volumes: vec![VolumeMount {
                volume: volume_name(request.name.as_str())?,
                target: info.data_path.parse()?,
                read_only: false,
            }],
            restart: Default::default(),
            limits: Default::default(),
            host_port: Some(host_port),
            database: Some(request.engine.clone()),
            uses_database: None,
            auto_deploy: false,
            require_approval: false,
            deploy_window: None,
            tag_trigger: None,
            store_app_id: None,
            targets: Vec::new(),
            load_balancing: None,
            alert_rules: None,
            sticky: None,
        },
    })
}

/// The volume name of a database: the app name and `-data`.
fn volume_name(app: &str) -> Result<VolumeName, Error> {
    format!("{app}-data").parse()
}

/// Seals the generated password as the secret environment variable of the engine. `None` for
/// an engine that takes no password, such as Redis.
async fn set_password(
    platform: &Arc<crate::platform::Platform>,
    app: &AppEntry,
    info: &EngineInfo,
) -> Result<Option<String>, AppError> {
    use r3v3rs3_api::platform::EnvEntry;
    if info.password_env.is_empty() {
        return Ok(None);
    }
    let password = format!("{:032x}", rand::random::<u128>());
    let entry = EnvEntry {
        key: info.password_env.parse().map_err(into_app_error)?,
        value: Some(password.clone()),
        secret: true,
    };
    platform
        .set_env(app.id, vec![entry])
        .await
        .map_err(platform_error)?;
    Ok(Some(password))
}

fn into_app_error(err: Error) -> AppError {
    AppError::R3v3rs3(err)
}
