//! The rate limit counts of the nodes in the cluster store.

use super::storage::KvStorage;
use crate::proxy::http::rate_share::{NodeCounts, RateCountExchange, RemoteCounts};
use tracing::warn;

#[async_trait::async_trait]
impl RateCountExchange for KvStorage {
    /// Writes one key for this node. The number of nodes comes from the presence keys.
    async fn exchange(&self, local: &NodeCounts) -> anyhow::Result<RemoteCounts> {
        let own = self.layout().rate_limit(&self.cluster_config().node_name);
        self.put_unconditional(own.clone(), &serde_json::to_vec(local)?, None)
            .await?;
        let list = self.store().list(&self.layout().rate_limits()).await?;
        let counts = list
            .items
            .iter()
            .filter(|item| item.key != own)
            .filter_map(|item| match self.decode_json::<NodeCounts>(item) {
                Ok(counts) => Some(counts),
                Err(err) => {
                    warn!(key = item.key, "invalid rate limit counts: {err:#}");
                    None
                }
            })
            .collect();
        let nodes = self.store().list(&self.layout().nodes()).await?.items.len();
        Ok(RemoteCounts { nodes, counts })
    }
}
