use super::RpcMethod;
use crate::accounts::Permission;
use crate::audit::AuditRecord;
use crate::server::state::ServerState;
use r3v3rs3_api::app::{AppConfig, WebhookConfig};
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;

pub struct GetConfig;

#[async_trait::async_trait]
impl RpcMethod for GetConfig {
    type Output = AppConfig;
    const PERMISSION: Permission = Permission::Admin;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.config().masked())
    }
}

pub struct SetConfig {
    pub config: AppConfig,
}

#[async_trait::async_trait]
impl RpcMethod for SetConfig {
    type Output = ();
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::Admin;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state.set_config(self.config).await
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::UpdateConfig))
    }
}

/// The notification webhook with its token and the name of the node. The admin API sends a test
/// notification with them outside the server loop, so a slow webhook does not delay the server.
pub struct GetNotificationWebhook;

#[async_trait::async_trait]
impl RpcMethod for GetNotificationWebhook {
    type Output = (Option<WebhookConfig>, String);
    const PERMISSION: Permission = Permission::Admin;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let config = state.config();
        let webhook = config.notifications.webhook.clone();
        Ok((webhook, config.cluster.node_name.clone()))
    }
}
