//! The cached responses and the cache purges of the proxies in the cluster store.

use super::layout::last_segment;
use super::storage::KvStorage;
use crate::kv::KvItem;
use crate::proxy::http::cache_share::{SharedCacheStore, SharedResponse};
use r3v3rs3_api::cluster::ClusterBackend;
use r3v3rs3_api::id::ShortId;
use std::collections::HashMap;
use tracing::warn;

/// The largest response that the node stores in etcd. etcd accepts requests up to 1.5 MiB, and
/// its JSON API sends the value in base64.
const ETCD_VALUE_LIMIT: u64 = 1024 * 1024;
/// The largest response that the node stores in Consul. Consul accepts values up to 512 KiB, and
/// its transaction API sends the value in base64.
const CONSUL_VALUE_LIMIT: u64 = 350 * 1024;

#[async_trait::async_trait]
impl SharedCacheStore for KvStorage {
    fn max_value_size(&self) -> u64 {
        let config = self.cluster_config();
        let limit = match config.backend {
            ClusterBackend::Etcd => ETCD_VALUE_LIMIT,
            ClusterBackend::Consul => CONSUL_VALUE_LIMIT,
        };
        config.cache_max_value_size.min(limit)
    }

    async fn put_response(
        &self,
        proxy: ShortId,
        key: &str,
        response: &SharedResponse,
    ) -> anyhow::Result<()> {
        let key = self.layout().cached_response(proxy, key);
        self.put_unconditional(key, &serde_json::to_vec(response)?, None)
            .await
    }

    async fn get_response(
        &self,
        proxy: ShortId,
        key: &str,
    ) -> anyhow::Result<Option<SharedResponse>> {
        let key = self.layout().cached_response(proxy, key);
        match self.store().get(&key).await? {
            Some(item) => Ok(Some(self.decode_json(&item)?)),
            None => Ok(None),
        }
    }
}

impl KvStorage {
    /// Records the purge time of the proxy, so every node purges its cache, and deletes the shared
    /// responses of the proxy.
    pub async fn purge_cache(&self, proxy: ShortId, at: u64) -> anyhow::Result<()> {
        let marker = self.layout().cache_purge(proxy);
        self.put_unconditional(marker, at.to_string().as_bytes(), None)
            .await?;
        self.delete_stale(&self.layout().cache(proxy), |_| true)
            .await
    }

    /// The last purge time of each proxy. An invalid purge key is left out.
    pub async fn cache_purges(&self) -> anyhow::Result<HashMap<ShortId, u64>> {
        let list = self.store().list(&self.layout().cache_purges()).await?;
        let purges = list
            .items
            .iter()
            .filter_map(|item| match self.cache_purge(item) {
                Ok(purge) => Some(purge),
                Err(err) => {
                    warn!(key = item.key, "invalid cache purge: {err:#}");
                    None
                }
            })
            .collect();
        Ok(purges)
    }

    fn cache_purge(&self, item: &KvItem) -> anyhow::Result<(ShortId, u64)> {
        let proxy = last_segment(&item.key).parse()?;
        let at = String::from_utf8(self.decode(item)?)?.parse()?;
        Ok((proxy, at))
    }

    /// Deletes the shared responses that no node uses anymore, and the responses that do not open.
    pub async fn remove_expired_responses(&self, now_ms: u64) -> anyhow::Result<()> {
        let prefix = self.layout().caches();
        self.delete_stale(&prefix, |item| {
            self.decode_json::<SharedResponse>(item)
                .map_or(true, |response| response.expires_at() <= now_ms)
        })
        .await
    }
}
