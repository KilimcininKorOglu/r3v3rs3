use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::proxies::GetProxy;
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use r3v3rs3_api::{
    error::Error,
    id::ShortId,
    log::{LogLevel, LogQuery, SystemLogRow},
};
use sqlx::ConnectOptions;
use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions};
use std::time::Duration;
use time::OffsetDateTime;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_INTERVAL: Duration = Duration::from_secs(1);
const REQUEST_DEFAULT_LIMIT: u32 = r3v3rs3_api::log::DEFAULT_LOG_QUERY_LIMIT;
const REQUEST_MAX_LIMIT: u32 = r3v3rs3_api::log::MAX_LOG_QUERY_LIMIT;

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

        let since_str = format_since(since);
        let until_str = format_until(until);
        let effective_limit = limit
            .unwrap_or(REQUEST_DEFAULT_LIMIT)
            .min(REQUEST_MAX_LIMIT);

        loop {
            let rows = sqlx::query("select * from system_log WHERE resource_id = ? AND (timestamp BETWEEN ? AND ?) ORDER BY timestamp DESC LIMIT ?")
                .bind(resource_id)
                .bind(&since_str)
                .bind(&until_str)
                .bind(effective_limit)
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

fn format_since(since: Option<OffsetDateTime>) -> String {
    let dt = since.unwrap_or(OffsetDateTime::UNIX_EPOCH);
    if dt.nanosecond() == 0 {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            dt.year(),
            u8::from(dt.month()),
            dt.day(),
            dt.hour(),
            dt.minute(),
            dt.second()
        )
    } else {
        dt.format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00".to_string())
    }
}

fn format_until(until: Option<OffsetDateTime>) -> String {
    match until {
        Some(dt) if dt.nanosecond() == 0 => {
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}~",
                dt.year(),
                u8::from(dt.month()),
                dt.day(),
                dt.hour(),
                dt.minute(),
                dt.second()
            )
        }
        Some(dt) => dt
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "9999-12-31T23:59:59~".to_string()),
        None => "9999-12-31T23:59:59~".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month, Time};

    #[tokio::test]
    async fn fetch_system_log_includes_rows_of_same_second() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE system_log (
                timestamp INTEGER NOT NULL,
                level TINYINT NOT NULL,
                resource_id STRING,
                message STRING,
                fields STRING
            );",
        )
        .execute(&pool)
        .await
        .unwrap();

        let dt1 = Date::from_calendar_date(2023, Month::November, 14)
            .unwrap()
            .with_time(Time::from_hms_nano(22, 13, 20, 100_000_000).unwrap())
            .assume_utc();
        let dt2 = Date::from_calendar_date(2023, Month::November, 14)
            .unwrap()
            .with_time(Time::from_hms_nano(22, 13, 20, 800_000_000).unwrap())
            .assume_utc();

        sqlx::query("INSERT INTO system_log VALUES (?, 3, 'res1', 'msg1', '{}')")
            .bind(dt1)
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("INSERT INTO system_log VALUES (?, 3, 'res1', 'msg2', '{}')")
            .bind(dt2)
            .execute(&pool)
            .await
            .unwrap();

        let reader = LogReader { pool };
        let since = Date::from_calendar_date(2023, Month::November, 14)
            .unwrap()
            .with_time(Time::from_hms(22, 13, 20).unwrap())
            .assume_utc();

        let rows = reader
            .fetch_system_log("res1", Some(since), Some(since), None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].message, "msg1");
        assert_eq!(rows[1].message, "msg2");
    }

    #[tokio::test]
    async fn fetch_system_log_caps_unbounded_limit() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE system_log (
                timestamp INTEGER NOT NULL,
                level TINYINT NOT NULL,
                resource_id STRING,
                message STRING,
                fields STRING
            );",
        )
        .execute(&pool)
        .await
        .unwrap();

        for i in 0..600 {
            let dt = Date::from_calendar_date(2023, Month::November, 14)
                .unwrap()
                .with_time(Time::from_hms(10, 0, 0).unwrap())
                .assume_utc()
                + time::Duration::seconds(i);
            sqlx::query("INSERT INTO system_log VALUES (?, 3, 'res1', 'msg', '{}')")
                .bind(dt)
                .execute(&pool)
                .await
                .unwrap();
        }

        let reader = LogReader { pool };
        let until = Date::from_calendar_date(2023, Month::November, 14)
            .unwrap()
            .with_time(Time::from_hms(11, 0, 0).unwrap())
            .assume_utc();

        let rows = reader
            .fetch_system_log("res1", None, Some(until), Some(10_000))
            .await
            .unwrap();
        assert_eq!(rows.len(), REQUEST_MAX_LIMIT as usize);

        let rows_default = reader
            .fetch_system_log("res1", None, Some(until), None)
            .await
            .unwrap();
        assert_eq!(rows_default.len(), REQUEST_DEFAULT_LIMIT as usize);
    }
}
