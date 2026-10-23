use super::RpcMethod;
use crate::accounts::Permission;
use crate::audit::AuditRecord;
use crate::server::state::ServerState;
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::quarantine::{AutoQuarantineConfig, QuarantinedIpEntry};
use std::net::IpAddr;

/// Retrieves the list of currently quarantined IP addresses.
pub struct GetQuarantinedList;

#[async_trait::async_trait]
impl RpcMethod for GetQuarantinedList {
    type Output = Vec<QuarantinedIpEntry>;
    const PERMISSION: Permission = Permission::Read;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.registries.quarantine.list_quarantined())
    }
}

/// Manually quarantines an IP address.
pub struct QuarantineIp {
    pub ip: IpAddr,
    pub reason: String,
    pub duration_seconds: u64,
}

#[async_trait::async_trait]
impl RpcMethod for QuarantineIp {
    type Output = QuarantinedIpEntry;
    const PERMISSION: Permission = Permission::Edit;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let entry =
            state
                .registries
                .quarantine
                .quarantine(self.ip, self.reason, self.duration_seconds);
        Ok(entry)
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(
            AuditRecord::new(AuditAction::QuarantineIp)
                .summary(format!("{} - {}", self.ip, self.reason)),
        )
    }
}

/// Releases an IP address from quarantine.
pub struct UnquarantineIp {
    pub ip: IpAddr,
}

#[async_trait::async_trait]
impl RpcMethod for UnquarantineIp {
    type Output = bool;
    const PERMISSION: Permission = Permission::Edit;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.registries.quarantine.unban(&self.ip))
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::UnquarantineIp).summary(self.ip.to_string()))
    }
}

/// Retrieves the current auto-quarantine configuration.
pub struct GetQuarantineConfig;

#[async_trait::async_trait]
impl RpcMethod for GetQuarantineConfig {
    type Output = AutoQuarantineConfig;
    const PERMISSION: Permission = Permission::Read;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.registries.quarantine.get_config())
    }
}

/// Updates and persists the auto-quarantine configuration.
pub struct UpdateQuarantineConfig {
    pub config: AutoQuarantineConfig,
}

#[async_trait::async_trait]
impl RpcMethod for UpdateQuarantineConfig {
    type Output = ();
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::Edit;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        self.config.validate()?;
        let mut app_config = state.config().clone();
        app_config.auto_quarantine = self.config;
        state.set_config(app_config).await
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::UpdateQuarantineConfig))
    }
}
