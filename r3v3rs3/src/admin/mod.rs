use crate::accounts::{AccountDirectory, AccountEntry, Caller};
use crate::audit::{AuditLog, AuditRecord};
use crate::command::ServerCommand;
use crate::platform::PlatformHandle;
use crate::server::rpc::auth::{GetAuditLog, GetPlatform, GetSessionBackend};
use crate::server::rpc::config::GetConfig;
use crate::server::rpc::{ErasedRpcMethod, RpcCallback, RpcMethod, RpcWrapper};
use crate::sessions::{LocalSessions, SessionBackend};
use auth::LoginAttempts;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderValue, StatusCode, header::CACHE_CONTROL};
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
use r3v3rs3_api::app::{AppConfig, AppInfo};
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
    task::{Context, Poll, ready},
};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot, watch};
use tokio_stream::wrappers::{BroadcastStream, WatchStream};
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::{GovernorError, GovernorLayer};
use tracing::{trace, warn};
use utoipa::OpenApi;
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouterExt};
use utoipa_axum::routes;
use utoipa_swagger_ui::SwaggerUi;

mod access_lists;
mod accounts;
mod acme;
mod app_info;
mod audit;
pub(crate) mod auth;
mod cdn;
mod certs;
mod cluster;
mod config;
mod discovery;
mod git;
mod hooks;
mod logs;
mod oauth;
mod openapi;
mod platform;
mod ports;
mod proxies;
mod static_file;

pub async fn start_admin(
    app_info: AppInfo,
    addr: SocketAddr,
    command: mpsc::Sender<ServerCommand>,
    mut callback: mpsc::Receiver<RpcCallback>,
    event: broadcast::Sender<ServerEvent>,
    accounts: watch::Receiver<Arc<AccountDirectory>>,
) -> anyhow::Result<()> {
    let data = Data::new(app_info).await?;
    let data = Arc::new(Mutex::new(data));
    let app_state = AppState {
        sender: command,
        event: event.clone(),
        event_listener_counter: Arc::new(AtomicUsize::new(0)),
        accounts,
        data: data.clone(),
    };

    let data_clone = data.clone();
    tokio::spawn(async move {
        while let Some(cb) = callback.recv().await {
            let mut data = data_clone.lock().await;
            if let Some(tx) = data.rpc_callbacks.remove(&cb.id) {
                let _ = tx.send(cb.result);
            }
        }
    });

    load_server_state(&app_state).await?;

    let mut event_recv = event.subscribe();
    tokio::spawn(async move {
        loop {
            match event_recv.recv().await {
                Ok(ServerEvent::AppConfigUpdated { config }) => {
                    data.lock().await.config = *config;
                }
                Ok(ServerEvent::Shutdown) => break,
                Err(RecvError::Lagged(n)) => {
                    warn!("event stream lagged: {}", n);
                }
                _ => (),
            }
        }
    });

    let mut event_recv = event.subscribe();
    let app = admin_router(app_state)?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

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
    let mut data = app_state.data.lock().await;
    data.config = *config;
    data.sessions = *sessions;
    data.audit = Some(*audit);
    data.platform = *platform;
    Ok(())
}

/// Builds the admin router. The OpenAPI document and the Swagger UI require a session, like the
/// other endpoints under `/api` except sign-in and sign-out.
fn admin_router(app_state: AppState) -> anyhow::Result<Router> {
    let verify = middleware::from_fn_with_state(app_state.clone(), auth::verify);
    let (api, openapi) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .nest("/api", auth_routes()?)
        .nest("/api", resource_routes().route_layer(verify.clone()))
        // The webhooks and the OAuth callback carry a signature or a one-time state instead of a
        // session.
        .merge(public_routes()?)
        .split_for_parts();
    let docs: Router<AppState> =
        Router::from(SwaggerUi::new(DOCS_PATH).url(OPENAPI_PATH, openapi)).route_layer(verify);
    Ok(api
        .merge(docs)
        // Every admin response carries the config of the server, so no cache may keep it. The
        // layer sits above the fallback, which sets its own Cache-Control for the WebUI files.
        .layer(middleware::map_response(no_store))
        .fallback(static_file::fallback)
        .with_state(app_state))
}

async fn no_store(mut res: Response) -> Response {
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn auth_routes() -> anyhow::Result<OpenApiRouter<AppState>> {
    let governor_conf = GovernorConfigBuilder::default()
        .per_second(4)
        .burst_size(2)
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
        .routes(routes!(auth::login).layer(login_limit))
        .routes(routes!(auth::logout)))
}

/// The largest webhook event. A push of many commits stays far below it.
const MAX_HOOK_BODY: usize = 5 * 1024 * 1024;

/// The routes without a session. They share one rate limit per client address.
fn public_routes() -> anyhow::Result<OpenApiRouter<AppState>> {
    let governor_conf = GovernorConfigBuilder::default()
        .per_second(1)
        .burst_size(10)
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

fn resource_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(session_routes())
        .merge(traffic_routes())
        .merge(cert_routes())
        .merge(info_routes())
        .merge(platform_routes())
}

/// The targets and the apps of the deployment platform.
fn platform_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .nest(
            "/targets",
            OpenApiRouter::new()
                .routes(routes!(platform::list_targets, platform::add_target))
                .routes(routes!(platform::delete_target))
                .routes(routes!(platform::new_target_token)),
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
                .routes(routes!(platform::get_env, platform::put_env))
                .routes(routes!(platform::put_git_token, platform::delete_git_token))
                .routes(routes!(
                    platform::new_webhook_secret,
                    platform::delete_webhook_secret
                ))
                .routes(routes!(platform::list_deployments))
                .routes(routes!(platform::get_logs))
                .routes(routes!(platform::deploy_app)),
        )
        .nest(
            "/deployments",
            OpenApiRouter::new()
                .routes(routes!(platform::get_deployment))
                .routes(routes!(platform::rollback_deployment)),
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
                .routes(routes!(git::authorize_connection)),
        )
        .nest(
            "/platform/settings",
            OpenApiRouter::new().routes(routes!(git::get_settings, git::put_settings)),
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
            "/accounts",
            OpenApiRouter::new()
                .routes(routes!(accounts::list, accounts::add))
                .routes(routes!(accounts::put, accounts::delete)),
        )
}

/// The ports, the proxies and the access lists that carry the traffic.
fn traffic_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .nest("/ports", port_routes())
        .nest("/proxies", proxy_routes())
        .nest("/access_lists", access_list_routes())
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
        .routes(routes!(proxies::get, proxies::put, proxies::delete))
        .routes(routes!(proxies::status))
        .routes(routes!(proxies::purge_cache))
}

fn access_list_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(access_lists::list, access_lists::add))
        .routes(routes!(access_lists::put, access_lists::delete))
}

/// The certificates and the ACME accounts.
fn cert_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .nest("/certs", cert_file_routes())
        .nest("/acme", acme_routes())
}

fn cert_file_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(certs::list))
        .routes(routes!(certs::self_sign))
        .routes(routes!(certs::upload))
        .routes(routes!(certs::delete_many))
        .routes(routes!(certs::get, certs::delete))
        .routes(routes!(certs::download))
}

fn acme_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(acme::list, acme::add))
        .routes(routes!(acme::get, acme::put, acme::delete))
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
}

/// The views of the running server: build info, CDN ranges, discovery and cluster state.
fn state_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .nest(
            "/app_info",
            OpenApiRouter::new().routes(routes!(app_info::get)),
        )
        .nest(
            "/cdn",
            OpenApiRouter::new()
                .routes(routes!(cdn::get))
                .routes(routes!(cdn::refresh)),
        )
        .nest(
            "/discovery",
            OpenApiRouter::new().routes(routes!(discovery::list)),
        )
        .nest(
            "/cluster",
            OpenApiRouter::new().routes(routes!(cluster::status)),
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
) -> Sse<impl Stream<Item = Result<Event, axum::Error>>> {
    let stream = StreamWrapper::new(
        BroadcastStream::new(state.event.subscribe()),
        state.event_listener_counter.clone(),
        state.sender.clone(),
        caller,
        state.accounts.clone(),
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// The server events that the account of the stream sees. The stream ends when the account
/// changes or is removed.
struct StreamWrapper {
    inner: BroadcastStream<ServerEvent>,
    accounts: WatchStream<Arc<AccountDirectory>>,
    caller: Caller,
    /// The account at the start of the stream.
    account: Option<AccountEntry>,
    counter: Arc<AtomicUsize>,
    sender: Sender<ServerCommand>,
}

impl StreamWrapper {
    fn new(
        stream: BroadcastStream<ServerEvent>,
        counter: Arc<AtomicUsize>,
        sender: Sender<ServerCommand>,
        caller: Caller,
        accounts: watch::Receiver<Arc<AccountDirectory>>,
    ) -> Self {
        if counter.fetch_add(1, Ordering::Relaxed) == 0 {
            let _ = sender.try_send(ServerCommand::SetBroadcastEvents { enabled: true });
        }
        let account = accounts.borrow().get(&caller.username).cloned();
        Self {
            inner: stream,
            accounts: WatchStream::new(accounts),
            caller,
            account,
            counter,
            sender,
        }
    }

    fn account_changed(&mut self, cx: &mut Context<'_>) -> bool {
        while let Poll::Ready(directory) = self.accounts.poll_next_unpin(cx) {
            let Some(directory) = directory else {
                return true;
            };
            if directory.get(&self.caller.username) != self.account.as_ref() {
                return true;
            }
        }
        false
    }
}

impl Stream for StreamWrapper {
    type Item = Result<Event, axum::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.account_changed(cx) {
            return Poll::Ready(None);
        }
        loop {
            let event = match ready!(this.inner.poll_next_unpin(cx)) {
                Some(Ok(event)) => event,
                Some(Err(err)) => return Poll::Ready(Some(Err(axum::Error::new(err)))),
                None => return Poll::Ready(None),
            };
            if let Some(event) = this.caller.visible_event(event) {
                return Poll::Ready(Some(Event::default().json_data(event)));
            }
        }
    }
}

impl Drop for StreamWrapper {
    fn drop(&mut self) {
        if self.counter.fetch_sub(1, Ordering::Release) == 1 {
            let _ = self
                .sender
                .try_send(ServerCommand::SetBroadcastEvents { enabled: false });
        }
    }
}

pub enum AppError {
    NotFound,
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
            AppError::Anyhow(err) => {
                message = err.to_string();
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

#[derive(Clone)]
pub struct AppState {
    pub sender: mpsc::Sender<ServerCommand>,
    pub event: broadcast::Sender<ServerEvent>,
    pub event_listener_counter: Arc<AtomicUsize>,
    /// The accounts that the sessions belong to.
    pub accounts: watch::Receiver<Arc<AccountDirectory>>,
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
    pub log: Arc<LogReader>,
    /// The audit log of the server. The admin API reads it at its start.
    pub audit: Option<Arc<AuditLog>>,
    /// The deployment platform of the server. The admin API reads it at its start.
    pub platform: PlatformHandle,

    pub rpc_counter: usize,
    pub rpc_callbacks: HashMap<usize, oneshot::Sender<CallbackData>>,
}

impl Data {
    pub async fn new(app_info: AppInfo) -> anyhow::Result<Self> {
        let log = app_info.log_path.join("log.db");
        Ok(Self {
            app_info,
            config: AppConfig::default(),
            sessions: Arc::new(LocalSessions::default()),
            login_attempts: Default::default(),
            log: Arc::new(LogReader::new(&log).await?),
            audit: None,
            platform: PlatformHandle::default(),
            rpc_counter: 0,
            rpc_callbacks: HashMap::new(),
        })
    }
}
