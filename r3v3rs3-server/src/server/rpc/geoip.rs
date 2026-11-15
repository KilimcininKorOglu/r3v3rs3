use super::RpcMethod;
use crate::accounts::Permission;
use crate::audit::AuditRecord;
use crate::server::state::ServerState;
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::geoip::{GeoIpStatus, UpdateGeoIpConfigRequest};
use r3v3rs3_api::webhook::WebhookEvent;

/// Retrieves the status and metadata of the currently loaded GeoIP database.
pub struct GetGeoIpStatus;

#[async_trait::async_trait]
impl RpcMethod for GetGeoIpStatus {
    type Output = GeoIpStatus;
    const PERMISSION: Permission = Permission::Read;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state
            .registries
            .geoip
            .status(state.config_dir(), &state.config().geoip))
    }
}

/// Updates the GeoIP configuration.
pub struct UpdateGeoIpConfig {
    pub request: UpdateGeoIpConfigRequest,
}

#[async_trait::async_trait]
impl RpcMethod for UpdateGeoIpConfig {
    type Output = GeoIpStatus;
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::Admin;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        self.request.validate()?;
        let mut app_config = state.config().clone();
        if let Some(account_id) = self.request.account_id {
            app_config.geoip.account_id = if account_id.trim().is_empty() {
                None
            } else {
                Some(account_id.trim().to_string())
            };
        }
        if let Some(license_key) = self.request.license_key {
            app_config.geoip.license_key = if license_key.trim().is_empty() {
                None
            } else {
                Some(license_key.trim().to_string())
            };
        }
        if let Some(database) = self.request.database {
            app_config.geoip.database = Some(database);
        }
        app_config.geoip.auto_update = self.request.auto_update;
        app_config.geoip.update_interval_days = self.request.update_interval_days;

        state.set_config(app_config).await?;
        Ok(state
            .registries
            .geoip
            .status(state.config_dir(), &state.config().geoip))
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::UpdateGeoIpConfig))
    }
}

/// Triggers a download and hot-reload of the latest MaxMind GeoLite2 database.
pub struct RefreshGeoIpDatabase;

#[async_trait::async_trait]
impl RpcMethod for RefreshGeoIpDatabase {
    type Output = GeoIpStatus;
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::Edit;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let config = state.config().geoip.clone();
        let status_res = state
            .registries
            .geoip
            .update_database(state.config_dir(), &config)
            .await;

        let platform = state.platform();
        match status_res {
            Ok(status) => {
                let summary = "GeoIP database updated successfully".to_string();
                let payload = serde_json::json!({
                    "loaded": status.loaded,
                    "path": status.database_path,
                    "size_bytes": status.file_size_bytes,
                    "build_epoch": status.build_epoch,
                });
                platform
                    .dispatch_webhook_event(WebhookEvent::SystemGeoipUpdated, summary, payload)
                    .await;
                Ok(status)
            }
            Err(err) => {
                let summary = format!("GeoIP database update failed: {err}");
                let payload = serde_json::json!({
                    "error": err.to_string(),
                });
                platform
                    .dispatch_webhook_event(WebhookEvent::SystemGeoipUpdateFailed, summary, payload)
                    .await;
                Err(Error::GeoIpUpdateFailed {
                    reason: err.to_string(),
                })
            }
        }
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::RefreshGeoIpDatabase))
    }
}

/// Periodic background check that updates the GeoIP database only if auto_update is true
/// and the update_interval_days has elapsed.
pub struct PeriodicGeoIpUpdateCheck;

#[async_trait::async_trait]
impl RpcMethod for PeriodicGeoIpUpdateCheck {
    type Output = bool;
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::Admin;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let config = state.config().geoip.clone();
        if !config.auto_update || config.license_key.is_none() {
            return Ok(false);
        }
        let status = state.registries.geoip.status(state.config_dir(), &config);

        let interval_ms = (config.update_interval_days.max(1) as u64) * 86_400 * 1000;
        let now_ms = crate::clock::unix_ms();
        let last_updated = status.last_updated_ms.unwrap_or(0);

        if now_ms.saturating_sub(last_updated) < interval_ms {
            return Ok(false);
        }

        let platform = state.platform();
        match state
            .registries
            .geoip
            .update_database(state.config_dir(), &config)
            .await
        {
            Ok(new_status) => {
                let summary = "Automatic GeoIP database update succeeded".to_string();
                let payload = serde_json::json!({
                    "loaded": new_status.loaded,
                    "path": new_status.database_path,
                    "size_bytes": new_status.file_size_bytes,
                    "build_epoch": new_status.build_epoch,
                });
                platform
                    .dispatch_webhook_event(WebhookEvent::SystemGeoipUpdated, summary, payload)
                    .await;
                Ok(true)
            }
            Err(err) => {
                tracing::warn!(%err, "automatic GeoIP database update failed");
                let summary = format!("Automatic GeoIP database update failed: {err}");
                let payload = serde_json::json!({ "error": err.to_string() });
                platform
                    .dispatch_webhook_event(WebhookEvent::SystemGeoipUpdateFailed, summary, payload)
                    .await;
                Ok(false)
            }
        }
    }
}
