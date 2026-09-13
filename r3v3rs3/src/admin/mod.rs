use crate::command::ServerCommand;
use crate::server::rpc::config::GetConfig;
use crate::server::rpc::{ErasedRpcMethod, RpcCallback, RpcMethod, RpcWrapper};
use auth::{LoginAttempts, SessionStore};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{middleware, Json};
use axum::{
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Router,
};
use futures::{Stream, TryStreamExt};
use logs::LogReader;
use openapi::{ApiDoc, ErrorResponses, DOCS_PATH, OPENAPI_PATH};
use r3v3rs3_api::app::{AppConfig, AppInfo};
use r3v3rs3_api::error::{Error, ErrorMessage};
use r3v3rs3_api::event::ServerEvent;
use std::any::Any;
use std::collections::HashMap;
use std::{
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::Sender;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::{GovernorError, GovernorLayer};
use tracing::{trace, warn};
use utoipa::OpenApi;
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouterExt};
use utoipa_axum::routes;
use utoipa_swagger_ui::SwaggerUi;

mod acme;
mod app_info;
pub(crate) mod auth;
mod cdn;
mod certs;
mod config;
mod logs;
mod openapi;
mod ports;
mod proxies;
mod static_file;

pub async fn start_admin(
    app_info: AppInfo,
    addr: SocketAddr,
    command: mpsc::Sender<ServerCommand>,
    mut callback: mpsc::Receiver<RpcCallback>,
    event: broadcast::Sender<ServerEvent>,
) -> anyhow::Result<()> {
    let data = Data::new(app_info).await?;
    let data = Arc::new(Mutex::new(data));
    let app_state = AppState {
        sender: command,
        event: event.clone(),
        event_listener_counter: Arc::new(AtomicUsize::new(0)),
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

    // The server broadcasts its initial config before this listener subscribes,
    // so fetch it explicitly.
    let config = app_state
        .call(GetConfig)
        .await
        .map_err(|err| anyhow::anyhow!("failed to load app config: {err}"))?;
    data.lock().await.config = *config;

    let mut event_recv = event.subscribe();
    tokio::spawn(async move {
        loop {
            match event_recv.recv().await {
                Ok(ServerEvent::AppConfigUpdated { config }) => {
                    data.lock().await.config = config;
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

/// Builds the admin router. The OpenAPI document and the Swagger UI require a session, like the
/// other endpoints under `/api` except sign-in and sign-out.
fn admin_router(app_state: AppState) -> anyhow::Result<Router> {
    let verify = middleware::from_fn_with_state(app_state.clone(), auth::verify);
    let (api, openapi) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .nest("/api", auth_routes()?)
        .nest("/api", resource_routes().route_layer(verify.clone()))
        .split_for_parts();
    let docs: Router<AppState> =
        Router::from(SwaggerUi::new(DOCS_PATH).url(OPENAPI_PATH, openapi)).route_layer(verify);
    Ok(api
        .merge(docs)
        .fallback(static_file::fallback)
        .with_state(app_state))
}

fn auth_routes() -> anyhow::Result<OpenApiRouter<AppState>> {
    let governor_conf = GovernorConfigBuilder::default()
        .per_second(4)
        .burst_size(2)
        .error_handler(|error| match error {
            GovernorError::TooManyRequests { .. } => {
                AppError::R3v3rs3(Error::TooManyLoginAttempts).into_response()
            }
            _ => AppError::Anyhow(anyhow::anyhow!(error)).into_response(),
        })
        .finish()
        .ok_or_else(|| anyhow::anyhow!("invalid login rate limit config"))?;
    let login_limit = GovernorLayer {
        config: Arc::new(governor_conf),
    };
    Ok(OpenApiRouter::new()
        .routes(routes!(auth::login).layer(login_limit))
        .routes(routes!(auth::logout)))
}

fn resource_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .nest("/events", OpenApiRouter::new().routes(routes!(events)))
        .nest(
            "/config",
            OpenApiRouter::new().routes(routes!(config::get, config::put)),
        )
        .nest(
            "/ports",
            OpenApiRouter::new()
                .routes(routes!(ports::list, ports::add))
                .routes(routes!(ports::get, ports::put, ports::delete))
                .routes(routes!(ports::status))
                .routes(routes!(ports::reset))
                .routes(routes!(ports::interfaces)),
        )
        .nest(
            "/proxies",
            OpenApiRouter::new()
                .routes(routes!(proxies::list, proxies::add))
                .routes(routes!(proxies::get, proxies::put, proxies::delete))
                .routes(routes!(proxies::status))
                .routes(routes!(proxies::purge_cache)),
        )
        .nest(
            "/certs",
            OpenApiRouter::new()
                .routes(routes!(certs::list))
                .routes(routes!(certs::self_sign))
                .routes(routes!(certs::upload))
                .routes(routes!(certs::get, certs::delete))
                .routes(routes!(certs::download)),
        )
        .nest(
            "/acme",
            OpenApiRouter::new()
                .routes(routes!(acme::list, acme::add))
                .routes(routes!(acme::get, acme::put, acme::delete)),
        )
        .nest("/logs", OpenApiRouter::new().routes(routes!(logs::get)))
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
async fn events(State(state): State<AppState>) -> Sse<StreamWrapper> {
    let stream = StreamWrapper::new(
        BroadcastStream::new(state.event.subscribe()),
        state.event_listener_counter.clone(),
        state.sender.clone(),
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

struct StreamWrapper {
    inner: BroadcastStream<ServerEvent>,
    counter: Arc<AtomicUsize>,
    sender: Sender<ServerCommand>,
}

impl StreamWrapper {
    fn new(
        stream: BroadcastStream<ServerEvent>,
        counter: Arc<AtomicUsize>,
        sender: Sender<ServerCommand>,
    ) -> Self {
        if counter.fetch_add(1, Ordering::Relaxed) == 0 {
            let _ = sender.try_send(ServerCommand::SetBroadcastEvents { enabled: true });
        }
        Self {
            inner: stream,
            counter,
            sender,
        }
    }
}

impl Stream for StreamWrapper {
    type Item = Result<Event, BroadcastStreamRecvError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.try_poll_next_unpin(cx) {
            Poll::Ready(Some(Ok(event))) => {
                Poll::Ready(Some(Ok(Event::default().json_data(event).unwrap())))
            }
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
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
    pub data: Arc<Mutex<Data>>,
}

impl AppState {
    pub async fn call<T>(&self, method: T) -> Result<Box<T::Output>, Error>
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
            .send(ServerCommand::CallMethod { id, arg })
            .await;

        match rx.await {
            Ok(v) => match v {
                Ok(value) => value.downcast().map_err(|_| Error::FailedToInvokeRpc),
                Err(err) => Err(err),
            },
            Err(_) => Err(Error::FailedToInvokeRpc),
        }
    }
}

pub type CallbackData = Result<Box<dyn Any + Send + Sync>, Error>;

pub struct Data {
    pub app_info: AppInfo,
    pub config: AppConfig,
    pub sessions: SessionStore,
    pub login_attempts: LoginAttempts,
    pub log: Arc<LogReader>,

    pub rpc_counter: usize,
    pub rpc_callbacks: HashMap<usize, oneshot::Sender<CallbackData>>,
}

impl Data {
    pub async fn new(app_info: AppInfo) -> anyhow::Result<Self> {
        let log = app_info.log_path.join("log.db");
        Ok(Self {
            app_info,
            config: AppConfig::default(),
            sessions: Default::default(),
            login_attempts: Default::default(),
            log: Arc::new(LogReader::new(&log).await?),
            rpc_counter: 0,
            rpc_callbacks: HashMap::new(),
        })
    }
}
