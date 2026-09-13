use super::RpcMethod;
use crate::server::state::ServerState;
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::error::Error;

pub struct GetConfig;

#[async_trait::async_trait]
impl RpcMethod for GetConfig {
    type Output = AppConfig;

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

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state.set_config(self.config).await
    }
}
