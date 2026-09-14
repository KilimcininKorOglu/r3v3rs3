//! The etcd provider. It reads the keys under a prefix through the v3 HTTP API and follows their
//! changes with a watch stream.

use super::kv::{self, KvEntry};
use super::{Built, ProxyGroups, Reporter, Watch, DEBOUNCE, MIN_BACKOFF};
use crate::kv::etcd::{EtcdClient, KeyRange, KeyValue};
use crate::kv::http::ApiClient;
use base64::prelude::{Engine as _, BASE64_STANDARD};
use r3v3rs3_api::discovery::{DiscoveryProvider, EtcdDiscoveryConfig};
use std::convert::Infallible;
use std::time::Duration;

const PROVIDER: DiscoveryProvider = DiscoveryProvider::Etcd;

/// The keys as text. A key that is not UTF-8 text is an issue.
fn kv_entries(built: &mut Built, prefix: &str, kvs: Vec<KeyValue>) -> Vec<KvEntry> {
    let mut entries = Vec::new();
    for kv in kvs {
        let key = BASE64_STANDARD
            .decode(&kv.key)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok());
        match key {
            Some(key) => entries.push(KvEntry {
                key,
                value: kv.value,
            }),
            None => built.issue(prefix, format!("the key is not UTF-8 text: {}", kv.key)),
        }
    }
    entries
}

pub(super) struct Provider {
    client: EtcdClient,
    range: KeyRange,
    reporter: Reporter,
}

#[async_trait::async_trait]
impl Watch for Provider {
    fn reporter(&self) -> &Reporter {
        &self.reporter
    }

    async fn watch(&self, backoff: &mut Duration) -> anyhow::Result<Infallible> {
        self.follow(backoff).await
    }
}

impl Provider {
    pub(super) fn new(config: &EtcdDiscoveryConfig, client: ApiClient, reporter: Reporter) -> Self {
        Self {
            client: EtcdClient::new(client, &config.username, config.password.as_deref()),
            range: KeyRange::new(&config.prefix),
            reporter,
        }
    }

    /// Reads the keys, then waits until a key changes and reads again.
    async fn follow(&self, backoff: &mut Duration) -> anyhow::Result<Infallible> {
        self.client.authenticate().await?;
        loop {
            let revision = self.sync().await?;
            *backoff = MIN_BACKOFF;
            self.client.wait_for_change(&self.range, revision).await?;
            tokio::time::sleep(DEBOUNCE).await;
        }
    }

    /// Reads the keys and sends the proxies. Returns the revision of the read.
    async fn sync(&self) -> anyhow::Result<i64> {
        let range = self.client.range(&self.range).await?;
        let revision = range.revision();
        let mut built = Built::default();
        let mut groups = ProxyGroups::new(PROVIDER);
        let prefix = &self.range.prefix;
        let entries = kv_entries(&mut built, prefix, range.kvs);
        kv::add_kv(&mut built, &mut groups, &entries, prefix);
        built.proxies = groups.into_proxies();
        self.reporter.running(built).await?;
        Ok(revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_decoded_and_a_key_that_is_not_text_is_an_issue() {
        let web = Some(BASE64_STANDARD.encode("web"));
        let kvs = vec![
            KeyValue {
                key: BASE64_STANDARD.encode("apps/r3v3rs3/http/app/ports"),
                value: web.clone(),
                ..Default::default()
            },
            KeyValue {
                key: "%%%".into(),
                value: web,
                ..Default::default()
            },
        ];
        let mut built = Built::default();
        let entries = kv_entries(&mut built, "apps/r3v3rs3", kvs);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "apps/r3v3rs3/http/app/ports");
        assert_eq!(built.issues[0].message, "the key is not UTF-8 text: %%%");
    }
}
