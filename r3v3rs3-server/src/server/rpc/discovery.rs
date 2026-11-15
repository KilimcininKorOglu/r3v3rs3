use super::RpcMethod;
use crate::server::state::ServerState;
use r3v3rs3_api::discovery::DiscoveryStatus;
use r3v3rs3_api::error::Error;

pub struct GetDiscoveryStatus;

#[async_trait::async_trait]
impl RpcMethod for GetDiscoveryStatus {
    type Output = Vec<DiscoveryStatus>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.discovery_statuses())
    }
}
