use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::server::self_upgrade;
use axum::{Extension, Json, extract::State};
use r3v3rs3_api::app::{UpgradeStage, UpgradeStageResponse, UpgradeStarted};
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use tracing::error;

/// Installs the newest release of the server binary and starts the version that carries it.
/// The running server hands its listeners over and drains, so the upgrade does not drop the
/// connections. The panel reloads onto the new process.
#[utoipa::path(
    post,
    path = "/",
    tag = "upgrade",
    operation_id = "upgrade_server",
    responses(
        (status = 200, description = "The upgrade is installed and the new process starts.", body = UpgradeStarted),
        ErrorResponses
    )
)]
pub async fn upgrade(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<UpgradeStarted>, AppError> {
    caller.authorize(Permission::Admin)?;

    set_stage(&state, Some(UpgradeStage::CheckingRelease));
    let client = crate::cdn::fetch::build_client()
        .await
        .map_err(|err| log_upgrade_failure(&err, "the HTTP client of the upgrade did not start"))?;
    let json = self_upgrade::fetch_latest(&client)
        .await
        .map_err(|err| log_upgrade_failure(&err, "the release information could not be fetched"))?;
    let Some(release) =
        self_upgrade::parse_release(&String::from_utf8_lossy(&json), env!("CARGO_PKG_VERSION"))
            .map_err(|err| log_upgrade_failure(&err, "the release information is not usable"))?
    else {
        set_stage(&state, None);
        return Err(Error::NoNewRelease.into());
    };
    set_stage(&state, Some(UpgradeStage::Downloading));
    let archive = self_upgrade::download(&client, &release.url)
        .await
        .map_err(|err| log_upgrade_failure(&err, "the release archive could not be downloaded"))?;
    set_stage(&state, Some(UpgradeStage::Installing));
    if let Err(err) = self_upgrade::install(&release, &archive) {
        error!(%err, "the server upgrade could not be installed");
        set_stage(&state, None);
        return Err(Error::UpgradeFailed.into());
    }

    set_stage(&state, Some(UpgradeStage::HandingOver));
    // The signal loop of the server picks this up and hands the listeners to the new process.
    if state.upgrade.try_send(()).is_err() {
        set_stage(&state, None);
        return Err(Error::UpgradeFailed.into());
    }
    set_stage(&state, Some(UpgradeStage::Done));

    let record = AuditRecord::new(AuditAction::UpgradeServer);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(UpgradeStarted { tag: release.tag }))
}

/// Logs the cause of a failed upgrade step and maps it to the API error.
fn log_upgrade_failure(err: &anyhow::Error, message: &str) -> Error {
    error!(%err, "{message}");
    Error::UpgradeFailed
}

/// Returns the step of the upgrade that runs at the moment, for the upgrade dialog.
#[utoipa::path(
    get,
    path = "/status",
    tag = "upgrade",
    operation_id = "upgrade_status",
    responses((status = 200, description = "The current upgrade step.", body = UpgradeStageResponse), ErrorResponses)
)]
pub async fn status(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<UpgradeStageResponse>, AppError> {
    caller.authorize(Permission::Read)?;
    let stage = state
        .upgrade_stage
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .copied();
    Ok(Json(UpgradeStageResponse { stage }))
}

/// Stores the current step of the upgrade, or clears it.
fn set_stage(state: &AppState, stage: Option<UpgradeStage>) {
    *state
        .upgrade_stage
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = stage;
}
