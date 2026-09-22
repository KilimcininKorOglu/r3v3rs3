use super::RpcMethod;
use crate::accounts::Permission;
use crate::audit::AuditLog;
use crate::platform::PlatformHandle;
use crate::server::state::ServerState;
use crate::sessions::SessionBackend;
use r3v3rs3_api::{
    auth::{LoginRequest, LoginResponse},
    error::Error,
};
use std::sync::Arc;

/// The sessions that the admin API shares with the proxies, and in a cluster with the other nodes.
pub struct GetSessionBackend;

#[async_trait::async_trait]
impl RpcMethod for GetSessionBackend {
    type Output = Arc<dyn SessionBackend>;
    const PERMISSION: Permission = Permission::Admin;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.session_backend())
    }
}

/// The audit log, for the sign-ins and the other changes of the admin API outside the RPC methods.
pub struct GetAuditLog;

#[async_trait::async_trait]
impl RpcMethod for GetAuditLog {
    type Output = Arc<AuditLog>;
    const PERMISSION: Permission = Permission::Admin;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.audit_log())
    }
}

/// The deployment platform, which the admin API calls outside the server loop.
pub struct GetPlatform;

#[async_trait::async_trait]
impl RpcMethod for GetPlatform {
    type Output = PlatformHandle;
    const PERMISSION: Permission = Permission::Admin;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.platform())
    }
}

pub struct VerifyAccount {
    pub request: LoginRequest,
}

#[async_trait::async_trait]
impl RpcMethod for VerifyAccount {
    type Output = LoginResponse;
    const PERMISSION: Permission = Permission::Admin;

    /// A valid sign-in reads the accounts again, so an account that `add-user` created while the
    /// server runs gets a session.
    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let response = state.storage.verify_account(self.request).await?;
        state.reload_accounts().await;
        Ok(response)
    }
}
