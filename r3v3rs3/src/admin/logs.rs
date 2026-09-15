use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::proxies::GetProxy;
use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use r3v3rs3_api::{
    error::Error,
    id::ShortId,
    log::{LogLevel, LogQuery, SystemLogRow},
};
use sqlx::ConnectOptions;
use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use std::time::Duration;
use time::OffsetDateTime;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_INTERVAL: Duration = Duration::from_secs(1);
const REQUEST_DEFAULT_LIMIT: u32 = 100;

/// Returns the log rows of a port, a proxy or a certificate, oldest first. Without `until`, the
/// request waits up to 10 seconds for a new row.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "logs",
    operation_id = "get_logs",
    params(
        ("id" = String, Path, description = "Id of the port, the proxy or the certificate."),
        LogQuery
    ),
    responses((status = 200, description = "The log rows.", body = Vec<SystemLogRow>), NotFoundResponse, ErrorResponses)
)]
pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Vec<SystemLogRow>>, AppError> {
    ensure_visible(&state, &caller, &id).await?;
    let log = state.data.lock().await.log.clone();
    let rows = log
        .fetch_system_log(&id, query.since, query.until, query.limit)
        .await?;
    Ok(Json(rows))
}

/// Rejects the log of a proxy that the account does not see. Every account sees the logs of the
/// ports and the certificates.
async fn ensure_visible(state: &AppState, caller: &Caller, id: &str) -> Result<(), Error> {
    let Ok(proxy) = id.parse::<ShortId>() else {
        return Ok(());
    };
    if caller.can_see(proxy) {
        return Ok(());
    }
    match state.call_system(GetProxy { id: proxy }).await {
        Ok(_) => Err(Error::IdNotFound { id: id.to_string() }),
        Err(Error::IdNotFound { .. }) => Ok(()),
        Err(err) => Err(err),
    }
}

pub struct LogReader {
    pool: SqlitePool,
}

impl LogReader {
    pub async fn new(path: &std::path::Path) -> anyhow::Result<Self> {
        let opt = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .log_statements(log::LevelFilter::Trace);
        let pool = SqlitePool::connect_with(opt).await?;
        Ok(Self { pool })
    }

    pub async fn fetch_system_log(
        &self,
        resource_id: &str,
        since: Option<OffsetDateTime>,
        until: Option<OffsetDateTime>,
        limit: Option<u32>,
    ) -> Result<Vec<SystemLogRow>, Error> {
        let mut timeout = tokio::time::interval(REQUEST_TIMEOUT);
        timeout.tick().await;

        loop {
            let rows = sqlx::query("select * from system_log WHERE resource_id = ? AND (timestamp BETWEEN ? AND ?) ORDER BY timestamp DESC LIMIT ?")
                .bind(resource_id)
                .bind(since.unwrap_or(OffsetDateTime::UNIX_EPOCH))
                .bind(until.unwrap_or_else(OffsetDateTime::now_utc))
                .bind(limit.unwrap_or(REQUEST_DEFAULT_LIMIT))
                .fetch_all(&self.pool);
            tokio::select! {
                _ = timeout.tick() => {
                    break;
                }
                rows = rows => {
                    match rows {
                        Ok(rows) if !rows.is_empty() || until.is_some() => {
                            return Ok(rows
                                .into_iter()
                                .map(|row| SystemLogRow {
                                    timestamp: row.get(0),
                                    level: row.get::<'_, u8, _>(1).try_into().unwrap_or(LogLevel::Debug),
                                    resource_id: row.get(2),
                                    message: row.get(3),
                                    fields: serde_json::from_str(row.get(4)).unwrap_or_default(),
                                })
                                .rev()
                                .collect());
                        },
                        Err(_) => {
                            return Err(Error::FailedToFetchLog);
                        }
                        _ => {
                            tokio::time::sleep(REQUEST_INTERVAL).await;
                        }
                    }
                }
            }
        }

        Ok(vec![])
    }
}
