use super::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Status payload returned by orchestrator liveness and readiness probe endpoints.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ProbeResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Liveness probe endpoint. Returns 200 OK if the admin process is running.
#[utoipa::path(
    get,
    path = "/healthz",
    tag = "health",
    operation_id = "healthz",
    responses(
        (status = 200, description = "The server process is alive.", body = ProbeResponse)
    )
)]
pub async fn healthz() -> Json<ProbeResponse> {
    Json(ProbeResponse {
        status: "ok".to_string(),
        message: None,
    })
}

/// Readiness probe endpoint. Checks if the server loop and database are ready to handle requests.
#[utoipa::path(
    get,
    path = "/readyz",
    tag = "health",
    operation_id = "readyz",
    responses(
        (status = 200, description = "The server is ready to accept traffic.", body = ProbeResponse),
        (status = 503, description = "The server is not ready.", body = ProbeResponse)
    )
)]
pub async fn readyz(
    State(state): State<AppState>,
) -> Result<Json<ProbeResponse>, (StatusCode, Json<ProbeResponse>)> {
    check_readiness(&state).await.map(Json).map_err(|err| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ProbeResponse {
                status: "not_ready".to_string(),
                message: Some(err),
            }),
        )
    })
}

async fn check_server_loop(state: &AppState) -> Result<(), String> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        state.call_system(crate::server::rpc::ports::GetPortList),
    )
    .await
    {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(err)) => Err(format!("server state error: {err}")),
        Err(_) => Err("server loop timeout".to_string()),
    }
}

async fn check_storage(state: &AppState) -> Result<(), String> {
    let pool = {
        let data = state.data.lock().await;
        data.platform.pool()
    };
    if let Some(pool) = pool {
        sqlx::query("SELECT 1")
            .execute(&pool)
            .await
            .map(|_| ())
            .map_err(|err| format!("database error: {err}"))?;
    }
    Ok(())
}

async fn check_readiness(state: &AppState) -> Result<ProbeResponse, String> {
    check_server_loop(state).await?;
    check_storage(state).await?;
    Ok(ProbeResponse {
        status: "ready".to_string(),
        message: None,
    })
}
