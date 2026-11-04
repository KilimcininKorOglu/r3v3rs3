use crate::accounts::{AccountDirectory, AccountEntry, Caller};
use crate::audit::{AuditLog, AuditRecord};
use crate::command::ServerCommand;
use crate::platform::PlatformHandle;
use crate::server::rpc::auth::{GetAuditLog, GetPlatform, GetSessionBackend, GetSqlitePool};
use crate::server::rpc::config::GetConfig;
use crate::server::rpc::{ErasedRpcMethod, RpcCallback, RpcMethod, RpcWrapper};
use crate::sessions::{LocalSessions, SessionBackend};
use auth::LoginAttempts;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{
    HeaderName, HeaderValue, StatusCode,
    header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY},
};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json, middleware};
use axum::{
    Router,
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
};
use futures::{Stream, StreamExt};
use logs::LogReader;
use openapi::{ApiDoc, DOCS_PATH, ErrorResponses, OPENAPI_PATH};
use r3v3rs3_api::app::{AppConfig, AppInfo, UpgradeStage};
use r3v3rs3_api::error::{Error, ErrorMessage};
use r3v3rs3_api::event::ServerEvent;
use std::any::Any;
use std::collections::HashMap;
use std::{
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot, watch};
use tokio_stream::wrappers::{ReceiverStream, WatchStream};
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::{GovernorError, GovernorLayer};
use tracing::{error, trace, warn};
use utoipa::OpenApi;
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouterExt};
use utoipa_axum::routes;
use utoipa_swagger_ui::SwaggerUi;

mod access_lists;
mod accounts;
mod acme;
mod anomaly;
mod app_files;
mod app_info;
mod audit;
pub(crate) mod auth;
mod backups;
pub(crate) mod ban;
mod cdn;
mod certs;
mod config;
mod dashboard;
pub(crate) mod databases;
mod disaster_recovery;
mod discovery;
mod dns_credentials;
mod error_page_themes;
mod geoip;
mod git;
mod grpc;
pub(crate) mod health;
mod hooks;
mod logs;
mod mcp;
mod metrics;
mod oauth;
pub(crate) mod oauth2;
mod openapi;
mod platform;
mod ports;
mod proxies;
mod quarantine;
mod security_headers;
mod static_file;
mod store;
pub(crate) mod system_metrics;
mod traffic;
mod upgrade;
mod webhooks;
mod workers;

/// The scopes of the access token that a request to the MCP endpoint carried.
#[derive(Clone, Debug)]
pub struct McpScope(pub String);

pub async fn start_admin(
    app_info: AppInfo,
    addr: SocketAddr,
    command: mpsc::Sender<ServerCommand>,
    callback: mpsc::Receiver<RpcCallback>,
    event: broadcast::Sender<ServerEvent>,
    accounts: watch::Receiver<Arc<AccountDirectory>>,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // The tests that start the admin API never call the upgrade endpoint, so the receiver of
    // this channel stays unread and an upgrade request answers with an error.
    let (upgrade, _) = mpsc::channel(1);
    start_admin_on(
        app_info, listener, command, callback, event, accounts, upgrade,
    )
    .await
}

/// The admin API on a listener that the caller bound, for example one that an upgrade
/// inherited.
pub async fn start_admin_on(
    app_info: AppInfo,
    listener: tokio::net::TcpListener,
    command: mpsc::Sender<ServerCommand>,
    callback: mpsc::Receiver<RpcCallback>,
    event: broadcast::Sender<ServerEvent>,
    accounts: watch::Receiver<Arc<AccountDirectory>>,
    upgrade: mpsc::Sender<()>,
) -> anyhow::Result<()> {
    let data = Data::new(app_info).await?;
    let data = Arc::new(Mutex::new(data));
    let app_state = AppState {
        sender: command,
        event: event.clone(),
        event_listener_counter: Arc::new(AtomicUsize::new(0)),
        accounts,
        upgrade,
        upgrade_stage: Arc::new(std::sync::Mutex::new(None)),
        data: data.clone(),
    };

    spawn_rpc_callbacks(callback, data.clone());
    load_server_state(&app_state).await?;
    spawn_config_events(event.clone(), data.clone());
    spawn_login_attempts_pruner(data);

    let mut event_recv = event.subscribe();
    let config_dir = app_state.data.lock().await.app_info.config_path.clone();
    anomaly::ensure_anomaly_monitor_running(app_state.clone(), config_dir.clone());
    crate::certs::ct::ensure_ct_monitor_running(app_state.clone(), config_dir);
    let app = admin_router(app_state)?;

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        loop {
            let event = event_recv.recv().await;
            trace!("received server event: {:?}", event);
            match event {
                Ok(ServerEvent::Shutdown) => {
                    break;
                }
                Err(RecvError::Lagged(n)) => {
                    warn!("event stream lagged: {}", n);
                }
                _ => {}
            }
        }
    })
    .await?;
    Ok(())
}

fn spawn_rpc_callbacks(mut callback: mpsc::Receiver<RpcCallback>, data: Arc<Mutex<Data>>) {
    tokio::spawn(async move {
        while let Some(cb) = callback.recv().await {
            let mut data = data.lock().await;
            if let Some(tx) = data.rpc_callbacks.remove(&cb.id) {
                let _ = tx.send(cb.result);
            }
        }
    });
}

fn spawn_config_events(event: broadcast::Sender<ServerEvent>, data: Arc<Mutex<Data>>) {
    let mut event_recv = event.subscribe();
    tokio::spawn(async move {
        loop {
            match event_recv.recv().await {
                Ok(ServerEvent::AppConfigUpdated { config }) => {
                    handle_app_config_update(&data, *config).await;
                }
                Ok(ServerEvent::Shutdown) => break,
                Err(RecvError::Lagged(n)) => {
                    warn!("event stream lagged: {}", n);
                }
                _ => (),
            }
        }
    });
}

async fn init_mcp_auth(data: &mut Data, pool: Option<sqlx::SqlitePool>) {
    if data.mcp.is_persistent() {
        return;
    }
    let shared_pool = pool.or_else(|| data.platform.pool());
    if let Some(p) = shared_pool
        && let Ok(mcp) = oauth2::McpAuthorizations::from_pool(p).await
    {
        data.mcp = mcp;
        return;
    }
    let db_path = data
        .app_info
        .config_path
        .join(crate::platform::DATABASE_FILE);
    if (data.config.mcp.enabled || data.config.platform.enabled || db_path.exists())
        && let Ok(mcp) = oauth2::McpAuthorizations::open(&db_path).await
    {
        data.mcp = mcp;
    }
}

async fn handle_app_config_update(data: &Arc<Mutex<Data>>, config: AppConfig) {
    let mut data = data.lock().await;
    data.config = config;
    init_mcp_auth(&mut data, None).await;
}

/// Reads the config, the sessions and the audit log of the server. The server broadcasts its
/// initial config before the admin API subscribes, so the admin API reads it explicitly.
async fn load_server_state(app_state: &AppState) -> anyhow::Result<()> {
    let config = app_state
        .call_system(GetConfig)
        .await
        .map_err(|err| anyhow::anyhow!("failed to load app config: {err}"))?;
    let sessions = app_state
        .call_system(GetSessionBackend)
        .await
        .map_err(|err| anyhow::anyhow!("failed to load the sessions: {err}"))?;
    let audit = app_state
        .call_system(GetAuditLog)
        .await
        .map_err(|err| anyhow::anyhow!("failed to load the audit log: {err}"))?;
    let platform = app_state
        .call_system(GetPlatform)
        .await
        .map_err(|err| anyhow::anyhow!("failed to load the deployment platform: {err}"))?;
    let pool = app_state
        .call_system(GetSqlitePool)
        .await
        .ok()
        .and_then(|b| *b);
    let mut data = app_state.data.lock().await;
    data.config = *config;
    data.sessions = *sessions;
    data.audit = Some(*audit);
    data.platform = *platform;

    init_mcp_auth(&mut data, pool).await;
    Ok(())
}

/// Builds the admin router. The OpenAPI document and the Swagger UI require a session, like the
/// other endpoints under `/api` except sign-in and sign-out.
fn admin_router(app_state: AppState) -> anyhow::Result<Router> {
    let verify = middleware::from_fn_with_state(app_state.clone(), auth::verify);
    let register_governor = GovernorConfigBuilder::default()
        .per_second(2)
        .burst_size(5)
        .key_extractor(Prefix64KeyExtractor)
        .finish()
        .ok_or_else(|| anyhow::anyhow!("invalid mcp register rate limit config"))?;
    let register_limit =
        GovernorLayer::new(Arc::new(register_governor)).error_handler(|error| match error {
            GovernorError::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS.into_response(),
            _ => AppError::Anyhow(anyhow::anyhow!(error)).into_response(),
        });
    let token_governor = GovernorConfigBuilder::default()
        .per_second(5)
        .burst_size(10)
        .key_extractor(Prefix64KeyExtractor)
        .finish()
        .ok_or_else(|| anyhow::anyhow!("invalid mcp token rate limit config"))?;
    let token_limit =
        GovernorLayer::new(Arc::new(token_governor)).error_handler(|error| match error {
            GovernorError::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS.into_response(),
            _ => AppError::Anyhow(anyhow::anyhow!(error)).into_response(),
        });
    // The authorization endpoint redirects to the sign-in page when signed out; the consent
    // submission requires an active session. The token and metadata endpoints are public.
    let mcp_oauth = OpenApiRouter::new()
        .routes(routes!(oauth2::authorize))
        .routes(routes!(oauth2::consent).layer(verify.clone()))
        .routes(routes!(oauth2::token).layer(token_limit))
        .routes(routes!(oauth2::register).layer(register_limit));
    let (api, openapi) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .nest("/api", auth_routes()?)
        .nest("/api", resource_routes().route_layer(verify.clone()))
        // The webhooks and the OAuth callback carry a signature or a one-time state instead of a
        // session.
        .merge(public_routes(&app_state)?)
        .merge(
            OpenApiRouter::new()
                .routes(routes!(oauth2::authorization_server_metadata))
                .routes(routes!(oauth2::protected_resource_metadata))
                .nest("/oauth", mcp_oauth),
        )
        .merge(metrics_routes(&app_state))
        .merge(health_routes())
        .split_for_parts();
    let docs: Router<AppState> = Router::from(SwaggerUi::new(DOCS_PATH).url(OPENAPI_PATH, openapi))
        .route_layer(verify);
    let mcp_verify = middleware::from_fn_with_state(app_state.clone(), oauth2::verify_bearer);
    let mcp_service = mcp::StreamableHttpService::new(
        {
            let state = app_state.clone();
            move || Ok(mcp::McpServer::new(state.clone()))
        },
        mcp::LocalSessionManager::default().into(),
        Default::default(),
    );
    let mcp_router: Router<AppState> = Router::new()
        .nest_service("/api/mcp", mcp_service)
        .route_layer(mcp_verify);
    Ok(api
        .merge(mcp_router)
        .merge(docs)
        // Every admin response carries the config of the server, so no cache may keep it. The
        // layer sits above the fallback, which sets its own Cache-Control for the WebUI files.
        .layer(middleware::from_fn(map_extractor_rejections))
        .layer(middleware::map_response(no_store))
        .fallback(static_file::fallback)
        .with_state(app_state))
}

async fn map_extractor_rejections(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let res = next.run(req).await;
    let status = res.status();
    if status != StatusCode::BAD_REQUEST
        && status != StatusCode::UNPROCESSABLE_ENTITY
        && status != StatusCode::UNSUPPORTED_MEDIA_TYPE
    {
        return res;
    }

    let is_text_plain = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|val| val.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/plain"));

    if !is_text_plain {
        return res;
    }

    let (parts, body) = res.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 64 * 1024).await else {
        return Response::from_parts(parts, axum::body::Body::empty());
    };
    let text = String::from_utf8_lossy(&bytes);
    let message = sanitize_rejection_message(&text, status);
    (
        status,
        Json(ErrorMessage {
            message,
            error: None,
        }),
    )
        .into_response()
}

fn sanitize_rejection_message(text: &str, status: StatusCode) -> String {
    let lower = text.to_ascii_lowercase();
    if lower.contains("content-type: application/json") {
        "Expected request with Content-Type: application/json".to_string()
    } else if lower.contains("missing request body") {
        "Request body is required".to_string()
    } else if lower.contains("path parameter") {
        "Invalid path parameter in request URL".to_string()
    } else if lower.contains("query") {
        "Invalid query parameters in request URL".to_string()
    } else {
        sanitize_payload_rejection(text, &lower, status)
    }
}

fn sanitize_payload_rejection(text: &str, lower: &str, status: StatusCode) -> String {
    if lower.contains("failed to parse the request body as json")
        || lower.contains("expected value at")
        || lower.contains("syntax error")
    {
        "Malformed or invalid JSON payload".to_string()
    } else if let Some(field) = extract_missing_field(text) {
        format!("Missing required field: {field}")
    } else if lower.contains("failed to deserialize the request body")
        || lower.contains("invalid type")
        || status == StatusCode::UNPROCESSABLE_ENTITY
    {
        "Invalid field format or value in request payload".to_string()
    } else {
        "Invalid request payload".to_string()
    }
}

fn extract_missing_field(text: &str) -> Option<&str> {
    let pos = text.find("missing field `")?;
    let field = text[pos + 15..].split('`').next()?;
    if field.is_empty() {
        None
    } else {
        Some(field)
    }
}

async fn no_store(mut res: Response) -> Response {
    let headers = res.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    res
}

#[derive(Clone, Default)]
struct Prefix64KeyExtractor;

impl tower_governor::key_extractor::KeyExtractor for Prefix64KeyExtractor {
    type Key = IpAddr;

    fn extract<T>(&self, req: &axum::http::Request<T>) -> Result<Self::Key, GovernorError> {
        let ip = tower_governor::key_extractor::PeerIpKeyExtractor.extract(req)?;
        Ok(r3v3rs3_api::client_ip::canonical_client_ip(ip))
    }
}

fn auth_routes() -> anyhow::Result<OpenApiRouter<AppState>> {
    let governor_conf = GovernorConfigBuilder::default()
        .per_second(4)
        .burst_size(2)
        .key_extractor(Prefix64KeyExtractor)
        .finish()
        .ok_or_else(|| anyhow::anyhow!("invalid login rate limit config"))?;
    let login_limit =
        GovernorLayer::new(Arc::new(governor_conf)).error_handler(|error| match error {
            GovernorError::TooManyRequests { .. } => {
                AppError::R3v3rs3(Error::TooManyLoginAttempts).into_response()
            }
            _ => AppError::Anyhow(anyhow::anyhow!(error)).into_response(),
        });
    Ok(OpenApiRouter::new()
        .routes(routes!(auth::login).layer(login_limit.clone()))
        .routes(routes!(auth::logout))
        .nest(
            "/auth",
            OpenApiRouter::new().routes(routes!(auth::get_passkey_challenge).layer(login_limit)),
        ))
}

/// The largest webhook event. A push of many commits stays far below it.
const MAX_HOOK_BODY: usize = 5 * 1024 * 1024;

/// Maximum body limit for application file uploads (100MB).
const MAX_APP_FILE_BODY: usize = 100 * 1024 * 1024;

/// Maximum body limit for volume archives and disaster recovery bundles (512MB).
const MAX_ARCHIVE_BODY: usize = 512 * 1024 * 1024;

#[derive(Clone)]
struct ResolvedClientIpKeyExtractor {
    state: AppState,
}

impl ResolvedClientIpKeyExtractor {
    fn is_peer_trusted(&self, peer_ip: &IpAddr) -> bool {
        if peer_ip.is_loopback() {
            return true;
        }
        self.state
            .data
            .try_lock()
            .map(|d| {
                d.config
                    .admin
                    .trusted_proxies
                    .iter()
                    .any(|net| net.contains(peer_ip))
            })
            .unwrap_or(false)
    }
}

impl tower_governor::key_extractor::KeyExtractor for ResolvedClientIpKeyExtractor {
    type Key = IpAddr;

    fn extract<T>(&self, req: &axum::http::Request<T>) -> Result<Self::Key, GovernorError> {
        let peer_ip = tower_governor::key_extractor::PeerIpKeyExtractor.extract(req)?;
        let trusted = self.is_peer_trusted(&peer_ip);
        let ip = if trusted {
            auth::header_ip(req.headers(), "x-forwarded-for")
                .or_else(|| auth::header_ip(req.headers(), "x-real-ip"))
                .unwrap_or(peer_ip)
        } else {
            peer_ip
        };
        Ok(r3v3rs3_api::client_ip::canonical_client_ip(ip))
    }
}

/// The routes without a session. They share one rate limit per client address.
fn public_routes(app_state: &AppState) -> anyhow::Result<OpenApiRouter<AppState>> {
    let governor_conf = GovernorConfigBuilder::default()
        .per_second(1)
        .burst_size(10)
        .key_extractor(ResolvedClientIpKeyExtractor {
            state: app_state.clone(),
        })
        .finish()
        .ok_or_else(|| anyhow::anyhow!("invalid webhook rate limit config"))?;
    let limit = GovernorLayer::new(Arc::new(governor_conf)).error_handler(|error| match error {
        GovernorError::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS.into_response(),
        _ => AppError::Anyhow(anyhow::anyhow!(error)).into_response(),
    });
    let hooks = OpenApiRouter::new()
        .routes(routes!(hooks::app_hook).layer(limit.clone()))
        .layer(DefaultBodyLimit::max(MAX_HOOK_BODY));
    let oauth = OpenApiRouter::new().routes(routes!(oauth::git_callback).layer(limit));
    Ok(OpenApiRouter::new()
        .nest("/hooks", hooks)
        .nest("/oauth", oauth))
}

/// The Prometheus metrics route. It accepts an admin session or an MCP Bearer token.
fn metrics_routes(state: &AppState) -> OpenApiRouter<AppState> {
    let verify = middleware::from_fn_with_state(state.clone(), metrics::verify_metrics);
    OpenApiRouter::new().nest(
        "/api/metrics",
        OpenApiRouter::new()
            .routes(routes!(metrics::metrics).layer(verify.clone()))
            .routes(routes!(metrics::grafana_dashboard).layer(verify)),
    )
}

/// Unauthenticated orchestrator liveness and readiness probe routes.
fn health_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(health::healthz))
        .routes(routes!(health::readyz))
}

fn resource_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(session_routes())
        .merge(traffic_routes())
        .merge(cert_routes())
        .merge(info_routes())
        .merge(platform_routes())
        .nest(
            "/mcp",
            OpenApiRouter::new()
                .routes(routes!(oauth2::list_mcp_sessions))
                .routes(routes!(oauth2::revoke_mcp_session)),
        )
        .nest(
            "/upgrade",
            OpenApiRouter::new().routes(routes!(upgrade::upgrade, upgrade::status)),
        )
}

/// The targets and the apps of the deployment platform.
fn platform_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .nest(
            "/platform",
            OpenApiRouter::new()
                .routes(routes!(platform::rotate_platform_key))
                .routes(routes!(
                    platform::get_prune_policy,
                    platform::update_prune_policy
                ))
                .routes(routes!(platform::run_prune))
                .routes(routes!(platform::vacuum_platform_logs))
                .routes(routes!(
                    platform::get_node_alerts,
                    platform::update_node_alerts
                )),
        )
        .nest(
            "/compose",
            OpenApiRouter::new().routes(routes!(platform::resolve_compose_env)),
        )
        .nest(
            "/targets",
            OpenApiRouter::new()
                .routes(routes!(platform::list_targets, platform::add_target))
                .routes(routes!(platform::upgrade_all_targets))
                .routes(routes!(platform::delete_target))
                .routes(routes!(platform::new_target_token))
                .routes(routes!(platform::upgrade_target))
                .routes(routes!(platform::drain_target))
                .routes(routes!(platform::undrain_target))
                .routes(routes!(platform::set_target_region)),
        )
        .nest(
            "/registries",
            OpenApiRouter::new()
                .routes(routes!(platform::list_registries, platform::add_registry))
                .routes(routes!(
                    platform::update_registry,
                    platform::delete_registry
                )),
        )
        .nest(
            "/env-groups",
            OpenApiRouter::new()
                .routes(routes!(
                    platform::list_env_groups,
                    platform::create_env_group
                ))
                .routes(routes!(
                    platform::get_env_group,
                    platform::update_env_group,
                    platform::delete_env_group
                )),
        )
        .nest(
            "/webhooks",
            OpenApiRouter::new()
                .routes(routes!(webhooks::list_webhooks, webhooks::create_webhook))
                .routes(routes!(webhooks::test_webhook))
                .routes(routes!(
                    webhooks::get_webhook,
                    webhooks::update_webhook,
                    webhooks::delete_webhook
                )),
        )
        .nest(
            "/projects",
            OpenApiRouter::new()
                .routes(routes!(platform::list_projects, platform::create_project))
                .routes(routes!(
                    platform::get_project,
                    platform::update_project,
                    platform::delete_project
                ))
                .routes(routes!(platform::create_environment))
                .routes(routes!(
                    platform::get_environment,
                    platform::update_environment,
                    platform::delete_environment
                ))
                .routes(routes!(
                    platform::get_environment_env,
                    platform::put_environment_env
                ))
                .routes(routes!(platform::diff_environments))
                .routes(routes!(platform::sync_environment_env)),
        )
        .nest(
            "/apps",
            OpenApiRouter::new()
                .routes(routes!(platform::list_apps, platform::add_app))
                .routes(routes!(
                    platform::get_app,
                    platform::update_app,
                    platform::delete_app
                ))
                .routes(routes!(
                    platform::get_env,
                    platform::put_env,
                    platform::generate_app_env
                ))
                .routes(routes!(platform::put_git_token, platform::delete_git_token))
                .routes(routes!(
                    platform::new_webhook_secret,
                    platform::delete_webhook_secret
                ))
                .routes(routes!(platform::install_hook))
                .routes(routes!(platform::list_deployments))
                .routes(routes!(platform::get_app_dashboard))
                .routes(routes!(platform::get_app_metrics))
                .routes(routes!(platform::get_app_resource_diagnostics))
                .routes(routes!(platform::get_logs))
                .routes(routes!(platform::get_log_size))
                .routes(routes!(platform::vacuum_app_logs))
                .routes(routes!(platform::logs_live))
                .routes(routes!(platform::get_services))
                .routes(routes!(platform::get_topology))
                .routes(routes!(platform::deploy_app))
                .routes(routes!(platform::check_app_update))
                .routes(routes!(platform::upgrade_app_catalog))
                .routes(routes!(
                    platform::get_app_scheduled_commands,
                    platform::create_app_scheduled_command
                ))
                .routes(routes!(
                    platform::update_app_scheduled_command,
                    platform::delete_app_scheduled_command
                ))
                .routes(routes!(platform::run_app_scheduled_command))
                .routes(routes!(platform::terminal))
                .routes(routes!(app_files::list))
                .routes(routes!(app_files::download))
                .routes(routes!(app_files::upload).layer(DefaultBodyLimit::max(MAX_APP_FILE_BODY)))
                .routes(routes!(app_files::delete))
                .routes(routes!(
                    app_files::list_volume_backups,
                    app_files::create_volume_backup
                ))
                .routes(routes!(
                    app_files::download_volume_backup,
                    app_files::restore_volume_backup,
                    app_files::delete_volume_backup
                ))
                .routes(routes!(app_files::export_volume_archive))
                .routes(
                    routes!(app_files::import_volume_archive)
                        .layer(DefaultBodyLimit::max(MAX_ARCHIVE_BODY)),
                ),
        )
        .nest(
            "/deployments",
            OpenApiRouter::new()
                .routes(routes!(platform::get_deployment))
                .routes(routes!(platform::rollback_deployment))
                .routes(routes!(platform::approve_deployment))
                .routes(routes!(platform::reject_deployment))
                .routes(routes!(platform::cancel_deployment)),
        )
        .nest(
            "/databases",
            OpenApiRouter::new()
                .routes(routes!(databases::engines, databases::refresh_engines))
                .routes(routes!(databases::list, databases::create))
                .routes(routes!(backups::list_s3))
                .routes(routes!(backups::download_s3))
                .routes(routes!(backups::restore_s3))
                .routes(routes!(databases::delete))
                .routes(routes!(databases::clone))
                .routes(routes!(databases::list_replicas, databases::create_replica))
                .routes(routes!(backups::list, backups::create))
                .routes(routes!(backups::download))
                .routes(routes!(backups::delete))
                .routes(routes!(backups::restore))
                .routes(routes!(backups::test_backup))
                .routes(routes!(backups::get_pitr_range, backups::restore_pitr))
                .routes(routes!(backups::archive_wal, backups::list_wal_segments)),
        )
        .nest(
            "/backup-config",
            OpenApiRouter::new().routes(routes!(backups::get_config, backups::put_config)),
        )
        .nest(
            "/backups",
            OpenApiRouter::new()
                .routes(routes!(backups::list_all))
                .routes(routes!(backups::overview))
                .routes(routes!(backups::run_backup_now)),
        )
        .nest(
            "/disaster-recovery",
            OpenApiRouter::new()
                .routes(routes!(disaster_recovery::list, disaster_recovery::create))
                .routes(routes!(disaster_recovery::download))
                .routes(routes!(disaster_recovery::delete))
                .routes(routes!(disaster_recovery::restore))
                .routes(
                    routes!(disaster_recovery::restore_upload)
                        .layer(DefaultBodyLimit::max(MAX_ARCHIVE_BODY)),
                ),
        )
        .nest(
            "/git/connections",
            OpenApiRouter::new()
                .routes(routes!(git::list_connections, git::add_connection))
                .routes(routes!(
                    git::get_connection,
                    git::update_connection,
                    git::delete_connection
                ))
                .routes(routes!(git::authorize_connection))
                .routes(routes!(git::install_github_app))
                .routes(routes!(git::list_repositories))
                .routes(routes!(git::list_branches)),
        )
        .nest(
            "/git/github_apps",
            OpenApiRouter::new().routes(routes!(git::create_github_app)),
        )
}

/// The event stream, the session and the accounts of the caller.
fn session_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .nest("/events", OpenApiRouter::new().routes(routes!(events)))
        .nest(
            "/session",
            OpenApiRouter::new().routes(routes!(auth::session)),
        )
        .nest(
            "/auth",
            OpenApiRouter::new()
                .routes(routes!(auth::register_passkey))
                .routes(routes!(auth::delete_passkey))
                .routes(routes!(auth::list_tokens, auth::create_token))
                .routes(routes!(auth::delete_token)),
        )
        .nest(
            "/accounts",
            OpenApiRouter::new()
                .routes(routes!(accounts::list, accounts::add))
                .routes(routes!(accounts::put, accounts::delete)),
        )
        .nest(
            "/banned-ips",
            OpenApiRouter::new()
                .routes(routes!(ban::list_banned_ips))
                .routes(routes!(ban::unban_ip)),
        )
}

/// The ports, the proxies and the access lists that carry the traffic.
fn traffic_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .nest("/ports", port_routes())
        .nest("/proxies", proxy_routes())
        .nest("/access_lists", access_list_routes())
        .nest("/error-page-themes", error_page_theme_routes())
        .nest(
            "/traffic",
            OpenApiRouter::new()
                .routes(routes!(traffic::get))
                .routes(routes!(traffic::live))
                .routes(routes!(traffic::geo))
                .routes(routes!(traffic::replay))
                .routes(routes!(traffic::get_entry))
                .routes(routes!(traffic::list_connections))
                .routes(routes!(traffic::terminate_connection))
                .routes(routes!(traffic::priority_stats)),
        )
        .nest(
            "/grpc",
            OpenApiRouter::new()
                .routes(routes!(grpc::reflect_standalone))
                .routes(routes!(grpc::invoke_standalone))
                .routes(routes!(grpc::health_standalone)),
        )
        .nest(
            "/system/anomalies",
            OpenApiRouter::new()
                .routes(routes!(anomaly::get_snapshot))
                .routes(routes!(anomaly::get_rules, anomaly::update_rules))
                .routes(routes!(anomaly::dismiss)),
        )
        .nest(
            "/security/quarantine",
            OpenApiRouter::new()
                .routes(routes!(quarantine::get_config, quarantine::update_config))
                .routes(routes!(quarantine::list, quarantine::add))
                .routes(routes!(quarantine::delete)),
        )
        .nest(
            "/security/geoip",
            OpenApiRouter::new()
                .routes(routes!(geoip::status))
                .routes(routes!(geoip::update_config))
                .routes(routes!(geoip::update_database)),
        )
        .nest(
            "/security/headers",
            OpenApiRouter::new()
                .routes(routes!(security_headers::list_profiles))
                .routes(routes!(security_headers::analyze_headers)),
        )
}

fn port_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(ports::list, ports::add))
        .routes(routes!(ports::get, ports::put, ports::delete))
        .routes(routes!(ports::status))
        .routes(routes!(ports::reset))
        .routes(routes!(ports::interfaces))
}

fn proxy_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(proxies::list, proxies::add))
        .routes(routes!(proxies::dry_run))
        .routes(routes!(grpc::reflect_proxy))
        .routes(routes!(grpc::invoke_proxy))
        .routes(routes!(grpc::health_proxy))
        .routes(routes!(security_headers::get_proxy_security_headers))
        .routes(routes!(
            security_headers::apply_proxy_security_header_profile
        ))
        .routes(routes!(proxies::get, proxies::put, proxies::delete))
        .routes(routes!(proxies::get_priority, proxies::put_priority))
        .routes(routes!(proxies::status))
        .routes(routes!(proxies::purge_cache, proxies::purge_cache_post))
}

fn access_list_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(access_lists::list, access_lists::add))
        .routes(routes!(access_lists::put, access_lists::delete))
}

fn error_page_theme_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(error_page_themes::list, error_page_themes::add))
        .routes(routes!(error_page_themes::reset_defaults))
        .routes(routes!(
            error_page_themes::get,
            error_page_themes::put,
            error_page_themes::delete
        ))
}

/// The certificates, the ACME accounts and the named DNS credentials.
fn cert_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .nest("/certs", cert_file_routes())
        .nest("/acme", acme_routes())
        .nest("/dns_credentials", dns_credential_routes())
}

/// The named DNS credentials that the ACME entries reference.
fn dns_credential_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(dns_credentials::list, dns_credentials::add))
        .routes(routes!(
            dns_credentials::get,
            dns_credentials::put,
            dns_credentials::delete
        ))
}

fn cert_file_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(certs::list))
        .routes(routes!(certs::self_sign))
        .routes(routes!(certs::upload).layer(DefaultBodyLimit::max(MAX_APP_FILE_BODY)))
        .routes(routes!(certs::delete_many))
        .routes(routes!(certs::check_coverage))
        .routes(routes!(certs::verify_cname))
        .routes(routes!(certs::get_ct_config, certs::put_ct_config))
        .routes(routes!(certs::get_ct_status))
        .routes(routes!(certs::list_ct_entries))
        .routes(routes!(certs::check_ct_now))
        .routes(routes!(certs::dismiss_ct_alert))
        .routes(routes!(certs::get, certs::delete))
        .routes(routes!(certs::download))
}

fn acme_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(acme::list, acme::add))
        .routes(routes!(acme::get, acme::put, acme::delete))
        .routes(routes!(acme::order, acme::cname_instructions))
}

/// The read-only views of the server plus the settings and the audit log.
fn info_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .nest("/audit", OpenApiRouter::new().routes(routes!(audit::list)))
        .nest("/config", config_routes())
        .nest("/logs", OpenApiRouter::new().routes(routes!(logs::get)))
        .merge(state_routes())
}

fn config_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(config::get, config::put))
        .routes(routes!(config::test_notification))
        .routes(routes!(config::backup))
        .routes(routes!(config::restore).layer(DefaultBodyLimit::max(MAX_APP_FILE_BODY)))
}

/// The views of the running server: build info, CDN ranges, discovery and cluster state.
fn state_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .nest(
            "/app_info",
            OpenApiRouter::new().routes(routes!(app_info::get)),
        )
        .nest(
            "/dashboard",
            OpenApiRouter::new().routes(routes!(dashboard::get)),
        )
        .nest(
            "/cdn",
            OpenApiRouter::new()
                .routes(routes!(cdn::get))
                .routes(routes!(cdn::refresh)),
        )
        .nest(
            "/store",
            OpenApiRouter::new()
                .routes(routes!(store::get))
                .routes(routes!(store::refresh)),
        )
        .nest(
            "/discovery",
            OpenApiRouter::new().routes(routes!(discovery::list)),
        )
        .nest(
            "/workers",
            OpenApiRouter::new().routes(routes!(workers::list)),
        )
}

/// Streams the server events as Server-Sent Events. Each event carries one JSON `ServerEvent`.
#[utoipa::path(
    get,
    path = "/",
    tag = "events",
    operation_id = "subscribe_events",
    responses(
        (status = 200, description = "The event stream.", content_type = "text/event-stream", body = ServerEvent),
        ErrorResponses
    )
)]
async fn events(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    session_token: Option<Extension<auth::AuthSessionToken>>,
) -> Sse<impl Stream<Item = Result<Event, axum::Error>>> {
    let session_token = session_token.map(|Extension(t)| t.0);
    let stream = StreamWrapper::new(&state, caller, session_token);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// The server events that the account of the stream sees. The stream ends when the account
/// changes or is removed, or when its admin session ends or expires.
struct StreamWrapper {
    inner: ReceiverStream<Result<Event, axum::Error>>,
    counter: Arc<AtomicUsize>,
    sender: Sender<ServerCommand>,
}

fn send_broadcast_toggle(sender: Sender<ServerCommand>, enabled: bool) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            let _ = sender.send(ServerCommand::SetBroadcastEvents { enabled }).await;
        });
    }
}

impl StreamWrapper {
    fn new(state: &AppState, caller: Caller, session_token: Option<String>) -> Self {
        let counter = state.event_listener_counter.clone();
        let sender = state.sender.clone();
        if counter.fetch_add(1, Ordering::Relaxed) == 0 {
            send_broadcast_toggle(sender.clone(), true);
        }
        let initial_account = state.accounts.borrow().get(&caller.username).cloned();
        let (tx, rx) = mpsc::channel(32);
        let state_clone = state.clone();

        tokio::spawn(async move {
            run_event_stream(state_clone, caller, initial_account, session_token, tx).await;
        });

        Self {
            inner: ReceiverStream::new(rx),
            counter,
            sender,
        }
    }
}

async fn is_session_invalid(state: &AppState, token: &str) -> bool {
    let (backend, expiry) = auth::session_backend(state).await;
    match backend
        .get(crate::sessions::SessionScope::Admin, token)
        .await
    {
        Ok(Some(record)) => !record.is_active(expiry),
        _ => true,
    }
}

async fn handle_broadcast_event(
    state: &AppState,
    caller: &Caller,
    session_token: Option<&str>,
    event: ServerEvent,
    tx: &mpsc::Sender<Result<Event, axum::Error>>,
) -> bool {
    if let Some(token) = session_token
        && is_session_invalid(state, token).await
    {
        return false;
    }
    if let Some(event) = caller.visible_event(event) {
        return tx.send(Event::default().json_data(event)).await.is_ok();
    }
    true
}

async fn run_event_stream(
    state: AppState,
    caller: Caller,
    initial_account: Option<AccountEntry>,
    session_token: Option<String>,
    tx: mpsc::Sender<Result<Event, axum::Error>>,
) {
    let mut broadcast = state.event.subscribe();
    let mut accounts_stream = WatchStream::new(state.accounts.clone());
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Some(token) = session_token.as_deref()
                    && is_session_invalid(&state, token).await
                {
                    break;
                }
            }
            account = accounts_stream.next() => {
                let Some(directory) = account else { break; };
                if directory.get(&caller.username) != initial_account.as_ref() {
                    break;
                }
            }
            event = broadcast.recv() => {
                match event {
                    Ok(event) => {
                        if !handle_broadcast_event(
                            &state,
                            &caller,
                            session_token.as_deref(),
                            event,
                            &tx,
                        )
                        .await
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("event stream lagged, skipped {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

impl Stream for StreamWrapper {
    type Item = Result<Event, axum::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        this.inner.poll_next_unpin(cx)
    }
}

impl Drop for StreamWrapper {
    fn drop(&mut self) {
        if self.counter.fetch_sub(1, Ordering::Release) == 1 {
            send_broadcast_toggle(self.sender.clone(), false);
        }
    }
}

pub enum AppError {
    NotFound,
    BadRequest(String),
    Anyhow(anyhow::Error),
    R3v3rs3(r3v3rs3_api::error::Error),
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::Anyhow(err)
    }
}

impl From<r3v3rs3_api::error::Error> for AppError {
    fn from(err: r3v3rs3_api::error::Error) -> Self {
        AppError::R3v3rs3(err)
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "not found"),
            Self::BadRequest(err) => write!(f, "{err}"),
            Self::Anyhow(err) => write!(f, "{err}"),
            Self::R3v3rs3(err) => write!(f, "{err}"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let code;
        let message;
        let mut error = None;
        match self {
            AppError::NotFound => {
                message = "NOT_FOUND".to_string();
                code = StatusCode::NOT_FOUND;
            }
            AppError::BadRequest(err) => {
                message = err;
                code = StatusCode::BAD_REQUEST;
            }
            AppError::Anyhow(err) => {
                error!(%err, "internal server error");
                message = "Internal server error".to_string();
                code = StatusCode::INTERNAL_SERVER_ERROR;
            }
            AppError::R3v3rs3(err) => {
                message = err.to_string();
                code = StatusCode::from_u16(err.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                error = Some(err);
            }
        }
        (code, Json(ErrorMessage { message, error })).into_response()
    }
}

fn is_safe_filename_char(c: char) -> bool {
    c != '"' && c != ';' && c != '\\' && !c.is_control()
}

/// Builds a sanitized Content-Disposition attachment header value, stripping quotes, semicolons,
/// backslashes, and control characters to prevent header and parameter injection.
pub fn content_disposition_attachment(filename: &str, default_fallback: &str) -> String {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or(default_fallback);
    let sanitized: String = base.chars().filter(|&c| is_safe_filename_char(c)).collect();
    let trimmed = sanitized.trim();
    let safe_name = if trimmed.is_empty() {
        default_fallback
    } else {
        trimmed
    };
    format!("attachment; filename=\"{safe_name}\"")
}

#[derive(Clone)]
pub struct AppState {
    pub sender: mpsc::Sender<ServerCommand>,
    pub event: broadcast::Sender<ServerEvent>,
    pub event_listener_counter: Arc<AtomicUsize>,
    /// The accounts that the sessions belong to.
    pub accounts: watch::Receiver<Arc<AccountDirectory>>,
    /// Wakes the signal loop of the server for an upgrade that the endpoint installed.
    pub upgrade: mpsc::Sender<()>,
    /// The step of the upgrade that runs at the moment, for the status endpoint.
    pub upgrade_stage: Arc<std::sync::Mutex<Option<UpgradeStage>>>,
    pub data: Arc<Mutex<Data>>,
}

impl AppState {
    /// Runs an RPC method for the account of a request.
    pub async fn call<T>(&self, caller: &Caller, method: T) -> Result<Box<T::Output>, Error>
    where
        T: RpcMethod,
    {
        let mut data = self.data.lock().await;
        let id = data.rpc_counter;
        data.rpc_counter += 1;

        let (tx, rx) = oneshot::channel();
        data.rpc_callbacks.insert(id, tx);
        std::mem::drop(data);

        let arg = Box::new(RpcWrapper::new(method)) as Box<dyn ErasedRpcMethod>;
        let _ = self
            .sender
            .send(ServerCommand::CallMethod {
                id,
                arg,
                caller: caller.clone(),
            })
            .await;

        match rx.await {
            Ok(v) => match v {
                Ok(value) => value.downcast().map_err(|_| Error::FailedToInvokeRpc),
                Err(err) => Err(err),
            },
            Err(_) => Err(Error::FailedToInvokeRpc),
        }
    }

    /// Runs an RPC method for the admin API itself, outside the request of an account.
    pub async fn call_system<T>(&self, method: T) -> Result<Box<T::Output>, Error>
    where
        T: RpcMethod,
    {
        self.call(&Caller::system(), method).await
    }

    /// Records a change that the admin API makes outside the RPC methods, for example a sign-in.
    pub async fn record_audit(&self, username: &str, client: Option<IpAddr>, record: AuditRecord) {
        let audit = self.data.lock().await.audit.clone();
        if let Some(audit) = audit {
            audit.record(username, client, record);
        }
    }
}

pub type CallbackData = Result<Box<dyn Any + Send + Sync>, Error>;

pub struct Data {
    pub app_info: AppInfo,
    pub config: AppConfig,
    /// The server replaces these sessions with its own sessions at the start of the admin API.
    pub sessions: Arc<dyn SessionBackend>,
    pub login_attempts: LoginAttempts,
    pub totp_attempts: auth::TotpAttempts,
    pub passkey_challenges: auth::PasskeyChallenges,
    pub ban_tracker: ban::IpBanTracker,
    pub log: Arc<LogReader>,
    /// The audit log of the server. The admin API reads it at its start.
    pub audit: Option<Arc<AuditLog>>,
    /// The deployment platform of the server. The admin API reads it at its start.
    pub platform: PlatformHandle,

    pub rpc_counter: usize,
    pub rpc_callbacks: HashMap<usize, oneshot::Sender<CallbackData>>,
    /// The authorization codes and tokens of the MCP clients.
    pub mcp: oauth2::McpAuthorizations,
}

impl Data {
    pub async fn new(app_info: AppInfo) -> anyhow::Result<Self> {
        let log = app_info.log_path.join("log.db");
        Ok(Self {
            app_info,
            config: AppConfig::default(),
            sessions: Arc::new(LocalSessions::default()),
            login_attempts: Default::default(),
            totp_attempts: Default::default(),
            passkey_challenges: Default::default(),
            ban_tracker: Default::default(),
            log: Arc::new(LogReader::new(&log).await?),
            audit: None,
            platform: PlatformHandle::default(),
            rpc_counter: 0,
            rpc_callbacks: HashMap::new(),
            mcp: oauth2::McpAuthorizations::default(),
        })
    }
}

fn spawn_login_attempts_pruner(data: Arc<Mutex<Data>>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            let mut data = data.lock().await;
            let reset = data.config.admin.login_attempts_reset;
            let now = std::time::Instant::now();
            data.login_attempts.prune_expired(reset, now);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use tower_governor::key_extractor::KeyExtractor;

    #[tokio::test]
    async fn resolved_client_ip_extractor_uses_forwarded_header_behind_proxy() {
        let temp_dir = std::env::temp_dir().join(format!(
            "r3-webhook-rate-{}",
            r3v3rs3_api::id::ShortId::new()
        ));
        let _ = std::fs::create_dir_all(&temp_dir);
        let _ = crate::log::open_database(&temp_dir.join("log.db"))
            .await
            .unwrap();
        let app_info = crate::config::new_appinfo(&temp_dir, &temp_dir);
        let data = Arc::new(Mutex::new(Data::new(app_info).await.unwrap()));
        let (sender, _) = mpsc::channel(1);
        let (event, _) = broadcast::channel(1);
        let (_, accounts) = watch::channel(Arc::new(Default::default()));
        let (upgrade, _) = mpsc::channel(1);
        let state = AppState {
            sender,
            event,
            event_listener_counter: Arc::new(AtomicUsize::new(0)),
            accounts,
            upgrade,
            upgrade_stage: Arc::new(std::sync::Mutex::new(None)),
            data,
        };

        let extractor = ResolvedClientIpKeyExtractor { state };

        let req = Request::builder()
            .header("x-forwarded-for", "203.0.113.195, 10.0.0.1")
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))))
            .body(())
            .unwrap();
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 195)));

        let req = Request::builder()
            .header("x-real-ip", "198.51.100.5")
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))))
            .body(())
            .unwrap();
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, IpAddr::V4(Ipv4Addr::new(198, 51, 100, 5)));

        let req = Request::builder()
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))))
            .body(())
            .unwrap();
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));

        let req = Request::builder()
            .header("x-forwarded-for", "1.2.3.4")
            .extension(ConnectInfo(SocketAddr::from(([198, 51, 100, 10], 12345))))
            .body(())
            .unwrap();
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10)));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn app_error_anyhow_masks_internal_details_in_500_response() {
        let sensitive_err = anyhow::anyhow!("sqlite3 /var/data/r3v3rs3.db disk I/O error table users");
        let response = AppError::Anyhow(sensitive_err).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(!body_str.contains("disk I/O error"));
        assert!(!body_str.contains("/var/data/r3v3rs3.db"));
        assert!(body_str.contains("Internal server error"));
    }

    #[test]
    fn content_disposition_attachment_sanitizes_injected_parameters() {
        assert_eq!(
            content_disposition_attachment("/path/to/test.txt", "file"),
            "attachment; filename=\"test.txt\""
        );
        assert_eq!(
            content_disposition_attachment("test.txt\"; filename*=\"evil.sh", "file"),
            "attachment; filename=\"test.txt filename*=evil.sh\""
        );
        assert_eq!(
            content_disposition_attachment("evil\r\nname.txt", "file"),
            "attachment; filename=\"evilname.txt\""
        );
        assert_eq!(
            content_disposition_attachment(";;;\"\"\"", "fallback.txt"),
            "attachment; filename=\"fallback.txt\""
        );
    }

    #[tokio::test]
    async fn send_broadcast_toggle_delivers_reliably_even_when_channel_is_full() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(ServerCommand::SetBroadcastEvents { enabled: true })
            .unwrap();
        send_broadcast_toggle(tx, false);

        let first = rx.recv().await.unwrap();
        assert!(matches!(
            first,
            ServerCommand::SetBroadcastEvents { enabled: true }
        ));

        let second = rx.recv().await.unwrap();
        assert!(matches!(
            second,
            ServerCommand::SetBroadcastEvents { enabled: false }
        ));
    }
}
