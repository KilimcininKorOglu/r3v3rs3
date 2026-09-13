use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::{cdn, command::ServerCommand};
use axum::{extract::State, Json};
use r3v3rs3_api::{
    cdn::{CdnRangesSource, CdnStatus},
    error::Error,
};
use tracing::error;

/// Returns the source, the update time and the range counts of the CDN IP ranges.
#[utoipa::path(
    get,
    path = "/",
    tag = "cdn",
    operation_id = "get_cdn_status",
    responses((status = 200, description = "The CDN IP range status.", body = CdnStatus), ErrorResponses)
)]
pub async fn get() -> Json<CdnStatus> {
    Json(cdn::status())
}

/// Downloads the CDN IP ranges from the providers and applies them.
#[utoipa::path(
    post,
    path = "/refresh",
    tag = "cdn",
    operation_id = "refresh_cdn_ranges",
    responses((status = 200, description = "The new CDN IP range status.", body = CdnStatus), ErrorResponses)
)]
pub async fn refresh(State(state): State<AppState>) -> Result<Json<CdnStatus>, AppError> {
    let previous = cdn::table().ranges().clone();
    let result = match cdn::fetch::fetch_all(&previous).await {
        Ok(result) => result,
        Err(err) => {
            error!(%err, "failed to refresh CDN IP ranges");
            cdn::set_last_errors(vec![err.to_string()]);
            return Err(Error::FailedToRefreshCdnRanges.into());
        }
    };

    cdn::set_last_errors(result.errors.clone());
    let status = cdn::status_of(&result.ranges, CdnRangesSource::Downloaded, result.errors);
    state
        .sender
        .send(ServerCommand::SetCdnRanges {
            ranges: result.ranges,
        })
        .await
        .map_err(|_| Error::FailedToInvokeRpc)?;
    Ok(Json(status))
}
