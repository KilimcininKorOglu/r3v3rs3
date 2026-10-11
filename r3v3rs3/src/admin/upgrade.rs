use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::server::self_upgrade;
use axum::{Extension, Json, extract::State};
use r3v3rs3_api::app::UpgradeStarted;
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

    let client = crate::cdn::fetch::build_client()
        .await
        .map_err(|_| Error::UpgradeFailed)?;
    let json = self_upgrade::fetch_latest(&client)
        .await
        .map_err(|_| Error::UpgradeFailed)?;
    let Some(release) =
        self_upgrade::parse_release(&String::from_utf8_lossy(&json), env!("CARGO_PKG_VERSION"))
            .map_err(|_| Error::UpgradeFailed)?
    else {
        return Err(Error::NoNewRelease.into());
    };
    let archive = self_upgrade::download(&client, &release.url)
        .await
        .map_err(|_| Error::UpgradeFailed)?;
    if let Err(err) = self_upgrade::install(&release, &archive) {
        error!(%err, "the server upgrade could not be installed");
        return Err(Error::UpgradeFailed.into());
    }

    // The signal loop of the server picks this up and hands the listeners to the new process.
    state
        .upgrade
        .try_send(())
        .map_err(|_| Error::UpgradeFailed)?;

    let record = AuditRecord::new(AuditAction::UpgradeServer);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(UpgradeStarted { tag: release.tag }))
}
