use super::RpcMethod;
use crate::server::state::ServerState;
use r3v3rs3_api::app::WorkerStatusReport;
use r3v3rs3_api::error::Error;

/// Reports the workers that serve the ports of this master.
pub struct GetWorkers;

#[async_trait::async_trait]
impl RpcMethod for GetWorkers {
    type Output = WorkerStatusReport;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.workers_status())
    }
}
