use super::RpcMethod;
use crate::server::state::ServerState;
use r3v3rs3_api::cluster::ClusterStatus;
use r3v3rs3_api::error::Error;

pub struct GetClusterStatus;

#[async_trait::async_trait]
impl RpcMethod for GetClusterStatus {
    type Output = ClusterStatus;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.cluster_status())
    }
}
