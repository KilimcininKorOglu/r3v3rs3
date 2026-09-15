//! Cached responses that the nodes of a cluster share. A node queues a stored response for the
//! store and does not wait for the write. On a local miss, a node reads the store once with a short
//! timeout, so a slow store does not delay the request much.

use base64::{engine::general_purpose::STANDARD, Engine};
use r3v3rs3_api::id::ShortId;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// A response that stays fresh for a shorter time stays on the node that stored it.
pub const MIN_SHARED_FRESHNESS: Duration = Duration::from_secs(60);
/// The longest wait for the store on a local miss.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_millis(100);
/// The time that a shared response stays in the store after it becomes stale, so a node can
/// revalidate it.
const REVALIDATION_WINDOW: Duration = Duration::from_secs(3600);
const WRITE_QUEUE: usize = 64;
/// The bytes of a stored response besides its body and headers.
const ENTRY_OVERHEAD: u64 = 256;

/// A stored response in the form that the store keeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// The request header values of the header names in `Vary`.
    pub vary: Vec<(String, Option<String>)>,
    /// The age in seconds when the node received the response.
    pub initial_age: u64,
    /// The freshness lifetime in seconds.
    pub freshness: u64,
    /// The Unix time in milliseconds when the node stored the response.
    pub stored_at: u64,
    /// The body in base64.
    pub body: String,
}

impl SharedResponse {
    pub fn encode_body(body: &[u8]) -> String {
        STANDARD.encode(body)
    }

    pub fn decode_body(&self) -> anyhow::Result<Vec<u8>> {
        Ok(STANDARD.decode(&self.body)?)
    }

    /// The Unix time in milliseconds after which no node uses the response.
    pub fn expires_at(&self) -> u64 {
        let lifetime = Duration::from_secs(self.freshness.saturating_sub(self.initial_age));
        let lifetime = (lifetime + REVALIDATION_WINDOW).as_millis() as u64;
        self.stored_at.saturating_add(lifetime)
    }

    fn size(&self) -> u64 {
        let headers: usize = self
            .headers
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum();
        (self.body.len() + headers) as u64 + ENTRY_OVERHEAD
    }
}

#[async_trait::async_trait]
pub trait SharedCacheStore: Send + Sync {
    /// The largest response in bytes that the store keeps.
    fn max_value_size(&self) -> u64;

    async fn put_response(
        &self,
        proxy: ShortId,
        key: &str,
        response: &SharedResponse,
    ) -> anyhow::Result<()>;

    async fn get_response(
        &self,
        proxy: ShortId,
        key: &str,
    ) -> anyhow::Result<Option<SharedResponse>>;
}

/// The shared responses of the cache of one proxy.
pub struct CacheShare {
    proxy: ShortId,
    store: Arc<dyn SharedCacheStore>,
    writes: mpsc::Sender<(String, SharedResponse)>,
    /// The Unix time in milliseconds of the last purge. Older responses do not count.
    purged_at: AtomicU64,
}

impl fmt::Debug for CacheShare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CacheShare")
            .field("proxy", &self.proxy)
            .field("purged_at", &self.purged_at)
            .finish_non_exhaustive()
    }
}

impl CacheShare {
    /// Starts the task that writes the queued responses. The task stops when the share is dropped.
    pub fn new(proxy: ShortId, store: Arc<dyn SharedCacheStore>, purged_at: u64) -> Self {
        let (writes, queue) = mpsc::channel(WRITE_QUEUE);
        tokio::spawn(write_responses(proxy, store.clone(), queue));
        Self {
            proxy,
            store,
            writes,
            purged_at: AtomicU64::new(purged_at),
        }
    }

    /// Whether a response with this remaining freshness and body size can go to the store. The
    /// check runs before the node encodes the response.
    pub fn accepts(&self, remaining_freshness: Duration, body_len: usize) -> bool {
        let encoded_len = (body_len as u64).div_ceil(3) * 4;
        remaining_freshness >= MIN_SHARED_FRESHNESS
            && encoded_len + ENTRY_OVERHEAD <= self.store.max_value_size()
    }

    /// Queues a response for the other nodes. A full queue drops the response.
    pub fn offer(&self, key: &str, response: SharedResponse) {
        if response.size() > self.store.max_value_size() || response.stored_at < self.purged_at() {
            return;
        }
        if self.writes.try_send((key.to_string(), response)).is_err() {
            debug!(proxy = %self.proxy, "the shared cache queue is full, so a response stays local");
        }
    }

    /// The response in the store. `None` when the store has no response, when the response is
    /// older than the last purge, or when the store does not answer in [`LOOKUP_TIMEOUT`].
    pub async fn lookup(&self, key: &str) -> Option<SharedResponse> {
        let get = self.store.get_response(self.proxy, key);
        match tokio::time::timeout(LOOKUP_TIMEOUT, get).await {
            Ok(Ok(response)) => response.filter(|response| response.stored_at >= self.purged_at()),
            Ok(Err(err)) => {
                warn!(proxy = %self.proxy, "failed to read a shared response: {err:#}");
                None
            }
            Err(_) => {
                debug!(proxy = %self.proxy, "the store did not return a shared response in time");
                None
            }
        }
    }

    pub fn purge(&self, at: u64) {
        self.purged_at.fetch_max(at, Ordering::Relaxed);
    }

    fn purged_at(&self) -> u64 {
        self.purged_at.load(Ordering::Relaxed)
    }
}

async fn write_responses(
    proxy: ShortId,
    store: Arc<dyn SharedCacheStore>,
    mut queue: mpsc::Receiver<(String, SharedResponse)>,
) {
    while let Some((key, response)) = queue.recv().await {
        if let Err(err) = store.put_response(proxy, &key, &response).await {
            warn!(%proxy, "failed to share a cached response: {err:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryCache {
        responses: Mutex<HashMap<String, SharedResponse>>,
    }

    #[async_trait::async_trait]
    impl SharedCacheStore for MemoryCache {
        fn max_value_size(&self) -> u64 {
            1024
        }

        async fn put_response(
            &self,
            _proxy: ShortId,
            key: &str,
            response: &SharedResponse,
        ) -> anyhow::Result<()> {
            let mut responses = self.responses.lock().unwrap();
            responses.insert(key.to_string(), response.clone());
            Ok(())
        }

        async fn get_response(
            &self,
            _proxy: ShortId,
            key: &str,
        ) -> anyhow::Result<Option<SharedResponse>> {
            Ok(self.responses.lock().unwrap().get(key).cloned())
        }
    }

    fn response(stored_at: u64, body: &[u8]) -> SharedResponse {
        SharedResponse {
            status: 200,
            headers: vec![("cache-control".into(), "max-age=120".into())],
            vary: Vec::new(),
            initial_age: 20,
            freshness: 120,
            stored_at,
            body: SharedResponse::encode_body(body),
        }
    }

    #[test]
    fn a_response_expires_after_its_freshness_and_the_revalidation_window() {
        let response = response(1000, b"body");
        assert_eq!(response.expires_at(), 1000 + 100_000 + 3_600_000);
        assert_eq!(response.decode_body().unwrap(), b"body");
    }

    #[tokio::test]
    async fn only_fresh_small_responses_after_the_last_purge_are_shared() {
        let store = Arc::new(MemoryCache::default());
        let share = CacheShare::new("web".parse().unwrap(), store.clone(), 500);
        assert!(share.accepts(Duration::from_secs(60), 500));
        assert!(!share.accepts(Duration::from_secs(59), 10));
        assert!(!share.accepts(Duration::from_secs(600), 600));

        share.offer("old", response(400, b"old"));
        share.offer("large", response(600, &[0; 800]));
        share.offer("new", response(600, b"new"));
        let mut shared = None;
        for _ in 0..50 {
            shared = share.lookup("new").await;
            if shared.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(shared, Some(response(600, b"new")));
        assert_eq!(share.lookup("old").await, None);
        assert_eq!(share.lookup("large").await, None);

        share.purge(700);
        assert_eq!(share.lookup("new").await, None);
    }
}
