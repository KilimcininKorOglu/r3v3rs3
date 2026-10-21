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
        AppEntry, AppRequest, AppSource, AppSpec, DatabaseCloneRequest, DatabaseCreated,
        DatabaseReplicaRequest, DatabaseRequest, DeploymentTrigger, EngineInfo, LOCAL_TARGET,
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

/// Clones an existing database: creates a new database branch, deploys it, and restores
/// the data from the source database.
#[utoipa::path(
    post,
    path = "/{id}/clone",
    tag = "platform",
    operation_id = "clone_database",
    params(("id" = ShortId, Path, description = "Source database id.")),
    request_body = DatabaseCloneRequest,
    responses((status = 200, description = "The cloned database.", body = DatabaseCreated), NotFoundResponse, ErrorResponses)
)]
pub async fn clone(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<DatabaseCloneRequest>,
) -> Result<Json<DatabaseCreated>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let source = platform.app(id).await.map_err(platform_error)?;
    let info = source_engine_info(&source)?;
    let host_port = resolve_host_port(request.host_port)?;
    let app_req = clone_app_request(&source, &request, &info, host_port).map_err(into_app_error)?;
    let app = platform
        .add_app(app_req, unix_ms())
        .await
        .map_err(platform_error)?;
    let password = set_password(&platform, &app, &info).await?;
    platform
        .deploy(app.id, &caller.username, DeploymentTrigger::Manual)
        .await
        .map_err(platform_error)?;
    let _ = platform
        .restore_clone_data(id, app.id, password.as_deref().unwrap_or_default())
        .await;
    record_clone_audit(&state, &caller, &app, &source).await;
    Ok(Json(DatabaseCreated {
        app,
        password,
        host_port,
        user: info.user,
        database: info.default_db,
    }))
}

/// Lists the read replicas of a primary database.
#[utoipa::path(
    get,
    path = "/{id}/replicas",
    tag = "platform",
    operation_id = "list_database_replicas",
    params(("id" = ShortId, Path, description = "Primary database id.")),
    responses((status = 200, description = "The read replicas of the database.", body = [AppEntry]), NotFoundResponse, ErrorResponses)
)]
pub async fn list_replicas(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Vec<AppEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let _primary = platform.app(id).await.map_err(platform_error)?;
    let apps = platform.apps().await.map_err(platform_error)?;
    let replicas = apps
        .into_iter()
        .filter(|app| app.spec.replica_of == Some(id))
        .collect();
    Ok(Json(replicas))
}

/// Creates a read replica of an existing primary database.
#[utoipa::path(
    post,
    path = "/{id}/replicas",
    tag = "platform",
    operation_id = "create_database_replica",
    params(("id" = ShortId, Path, description = "Primary database id.")),
    request_body = DatabaseReplicaRequest,
    responses((status = 200, description = "The new read replica database.", body = DatabaseCreated), NotFoundResponse, ErrorResponses)
)]
pub async fn create_replica(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<DatabaseReplicaRequest>,
) -> Result<Json<DatabaseCreated>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let primary = platform.app(id).await.map_err(platform_error)?;
    if primary.spec.replica_of.is_some() {
        return Err(Error::InvalidContainerSpec {
            reason: "replicas can only be created from primary databases".to_string(),
        }
        .into());
    }
    let info = source_engine_info(&primary)?;
    let host_port = resolve_host_port(request.host_port)?;
    let app_req =
        replica_app_request(&primary, &request, &info, host_port).map_err(into_app_error)?;
    let app = platform
        .add_app(app_req, unix_ms())
        .await
        .map_err(platform_error)?;
    let password = sync_replica_password(&platform, &primary, &app, &info).await?;
    platform
        .deploy(app.id, &caller.username, DeploymentTrigger::Manual)
        .await
        .map_err(platform_error)?;
    record_replica_audit(&state, &caller, &app, &primary).await;
    Ok(Json(DatabaseCreated {
        app,
        password,
        host_port,
        user: info.user,
        database: info.default_db,
    }))
}

async fn sync_replica_password(
    platform: &Arc<crate::platform::Platform>,
    primary: &AppEntry,
    replica: &AppEntry,
    info: &EngineInfo,
) -> Result<Option<String>, AppError> {
    use r3v3rs3_api::platform::EnvEntry;
    if info.password_env.is_empty() {
        return Ok(None);
    }
    let primary_password = platform
        .open_password(primary.id, info)
        .await
        .map_err(platform_error)?;
    let entry = EnvEntry {
        key: info.password_env.parse().map_err(into_app_error)?,
        value: Some(primary_password.clone()),
        secret: true,
    };
    platform
        .set_env(replica.id, vec![entry])
        .await
        .map_err(platform_error)?;
    Ok(Some(primary_password))
}

fn replica_app_request(
    primary: &AppEntry,
    request: &DatabaseReplicaRequest,
    info: &EngineInfo,
    host_port: u16,
) -> Result<AppRequest, Error> {
    let version = match &primary.spec.source {
        AppSource::Image { image } => image.to_string(),
        _ => info.versions[0].clone(),
    };
    Ok(AppRequest {
        name: request.name.clone(),
        target: LOCAL_TARGET.parse()?,
        spec: AppSpec {
            source: AppSource::Image {
                image: version.parse()?,
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
            limits: request.limits,
            host_port: Some(host_port),
            database: primary.spec.database.clone(),
            bot_blocker: false,
            grpc_web: false,
            uses_database: None,
            replica_of: Some(primary.id),
            auto_deploy: false,
            require_approval: false,
            deploy_window: None,
            tag_trigger: None,
            store_app_id: None,
            targets: Vec::new(),
            load_balancing: None,
            alert_rules: None,
            sticky: None,
            git_config: None,
            env_groups: Vec::new(),
            preview: None,
            preview_of: None,
            buildkit: None,
        },
    })
}

async fn record_replica_audit(
    state: &AppState,
    caller: &Caller,
    app: &AppEntry,
    primary: &AppEntry,
) {
    let record = AuditRecord::new(AuditAction::CreateDatabaseReplica)
        .id(app.id)
        .summary(format!("{} replica of {}", app.name, primary.name));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
}

fn source_engine_info(source: &AppEntry) -> Result<EngineInfo, AppError> {
    let engine = source.spec.database.as_deref().ok_or_else(|| {
        AppError::R3v3rs3(Error::InvalidContainerSpec {
            reason: format!("the app {} is not a managed database", source.name),
        })
    })?;
    r3v3rs3_api::platform::engine_info(engine).ok_or_else(|| {
        AppError::R3v3rs3(Error::InvalidContainerSpec {
            reason: format!("the engine {engine} is not in the database catalog"),
        })
    })
}

fn resolve_host_port(port: Option<u16>) -> Result<u16, AppError> {
    match port {
        Some(p) => Ok(p),
        None => allocate_host_port().map_err(platform_error),
    }
}

async fn record_clone_audit(state: &AppState, caller: &Caller, app: &AppEntry, source: &AppEntry) {
    let record = AuditRecord::new(AuditAction::CloneDatabase)
        .id(app.id)
        .summary(format!("{} from {}", app.name, source.name));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
}

fn clone_app_request(
    source: &AppEntry,
    request: &DatabaseCloneRequest,
    info: &EngineInfo,
    host_port: u16,
) -> Result<AppRequest, Error> {
    let version = match &source.spec.source {
        AppSource::Image { image } => image.to_string(),
        _ => info.versions[0].clone(),
    };
    let db_request = DatabaseRequest {
        name: request.name.clone(),
        engine: info.engine.clone(),
        version,
        host_port: Some(host_port),
        limits: request.limits.unwrap_or(source.spec.limits),
    };
    app_request(&db_request, info, host_port)
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
            limits: request.limits,
            host_port: Some(host_port),
            database: Some(request.engine.clone()),
            bot_blocker: false,
            grpc_web: false,
            uses_database: None,
            replica_of: None,
            auto_deploy: false,
            require_approval: false,
            deploy_window: None,
            tag_trigger: None,
            store_app_id: None,
            targets: Vec::new(),
            load_balancing: None,
            alert_rules: None,
            sticky: None,
            git_config: None,
            env_groups: Vec::new(),
            preview: None,
            preview_of: None,
            buildkit: None,
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
