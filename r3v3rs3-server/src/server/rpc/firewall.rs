use super::RpcMethod;
use crate::server::state::ServerState;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::firewall::FirewallStatus;

/// Reports the host firewall table that r3v3rs3 keeps.
pub struct GetFirewallStatus;

impl RpcMethod for GetFirewallStatus {
    type Output = FirewallStatus;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.firewall_status())
    }
}
