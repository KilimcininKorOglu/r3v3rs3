use super::RpcMethod;
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

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.session_backend())
    }
}

pub struct VerifyAccount {
    pub request: LoginRequest,
}

#[async_trait::async_trait]
impl RpcMethod for VerifyAccount {
    type Output = LoginResponse;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state.storage.verify_account(self.request).await
    }
}
