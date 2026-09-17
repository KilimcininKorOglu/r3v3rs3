//! The keys of the cluster data below `{prefix}/v1/`.

use crate::sessions::SessionScope;
use r3v3rs3_api::cert::CertKind;
use r3v3rs3_api::id::ShortId;
use sha2::{Digest, Sha256};

/// The parts of the state that a node reads again after a change in the store, in the order that
/// the node applies them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StateKind {
    Config,
    Certs,
    Acmes,
    Ports,
    AccessLists,
    Proxies,
    Cdn,
    Challenges,
    CachePurges,
    Accounts,
}

impl StateKind {
    pub const ALL: [Self; 10] = [
        Self::Config,
        Self::Certs,
        Self::Acmes,
        Self::Ports,
        Self::AccessLists,
        Self::Proxies,
        Self::Cdn,
        Self::Challenges,
        Self::CachePurges,
        Self::Accounts,
    ];
}

/// The first key segment below `state/` of each part.
const STATE_PARTS: [(&str, StateKind); 10] = [
    ("config", StateKind::Config),
    ("certs", StateKind::Certs),
    ("acme", StateKind::Acmes),
    ("ports", StateKind::Ports),
    ("access-lists", StateKind::AccessLists),
    ("proxies", StateKind::Proxies),
    ("cdn", StateKind::Cdn),
    ("challenges", StateKind::Challenges),
    ("cache-purges", StateKind::CachePurges),
    ("accounts", StateKind::Accounts),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    data: String,
}

impl Layout {
    pub fn new(prefix: &str) -> Self {
        Self {
            data: super::data_prefix(prefix),
        }
    }

    pub fn data(&self) -> &str {
        &self.data
    }

    /// The version of the data layout. `cluster import` writes it last.
    pub fn schema(&self) -> String {
        format!("{}schema", self.data)
    }

    pub fn state(&self) -> String {
        format!("{}state/", self.data)
    }

    pub fn config(&self) -> String {
        format!("{}config", self.state())
    }

    pub fn ports(&self) -> String {
        format!("{}ports/", self.state())
    }

    pub fn proxies(&self) -> String {
        format!("{}proxies/", self.state())
    }

    /// The access lists. They hold password hashes and token digests, so they are encrypted.
    pub fn access_lists(&self) -> String {
        format!("{}access-lists/", self.state())
    }

    pub fn access_list(&self, id: ShortId) -> String {
        format!("{}{id}", self.access_lists())
    }

    pub fn certs(&self) -> String {
        format!("{}certs/", self.state())
    }

    pub fn cert(&self, kind: CertKind, id: ShortId) -> String {
        format!("{}{kind}/{id}", self.certs())
    }

    pub fn acmes(&self) -> String {
        format!("{}acme/", self.state())
    }

    pub fn acme(&self, id: ShortId) -> String {
        format!("{}{id}", self.acmes())
    }

    pub fn accounts(&self) -> String {
        format!("{}accounts/", self.state())
    }

    pub fn account(&self, name: &str) -> String {
        hex_key(self.accounts(), name)
    }

    pub fn cdn(&self) -> String {
        format!("{}cdn", self.state())
    }

    /// The lock of the leader. Its value is the name of the node that holds it.
    pub fn leader(&self) -> String {
        format!("{}lock/leader", self.data)
    }

    /// The ACME challenges that every node serves. The ACME server publishes their values, so
    /// they are not encrypted.
    pub fn challenges(&self) -> String {
        format!("{}challenges/", self.state())
    }

    pub fn http_challenges(&self) -> String {
        format!("{}http/", self.challenges())
    }

    pub fn tls_alpn_challenges(&self) -> String {
        format!("{}tls-alpn/", self.challenges())
    }

    pub fn http_challenge(&self, token: &str) -> String {
        hex_key(self.http_challenges(), token)
    }

    pub fn tls_alpn_challenge(&self, domain: &str) -> String {
        hex_key(self.tls_alpn_challenges(), domain)
    }

    /// The presence keys of the nodes. Each node attaches its key to its lease.
    pub fn nodes(&self) -> String {
        format!("{}nodes/", self.data)
    }

    pub fn node(&self, name: &str) -> String {
        hex_key(self.nodes(), name)
    }

    /// The keys where the nodes write the digest of the challenges that they serve.
    pub fn acks(&self) -> String {
        format!("{}acks/", self.data)
    }

    pub fn ack(&self, name: &str) -> String {
        hex_key(self.acks(), name)
    }

    /// The ack key of the node with this presence key.
    pub fn ack_of(&self, node_key: &str) -> String {
        format!("{}{}", self.acks(), last_segment(node_key))
    }

    /// The sessions of the admin API and the proxies. They are outside the state, so a new
    /// session does not reload the state of the nodes.
    pub fn sessions(&self) -> String {
        format!("{}sessions/", self.data)
    }

    pub fn session(&self, scope: SessionScope, token_digest: &str) -> String {
        format!("{}{}/{token_digest}", self.sessions(), scope.as_str())
    }

    /// The rate limit counts that each node publishes. They hold client IP addresses, so they are
    /// encrypted.
    pub fn rate_limits(&self) -> String {
        format!("{}ratelimit/", self.data)
    }

    pub fn rate_limit(&self, node: &str) -> String {
        hex_key(self.rate_limits(), node)
    }

    /// The cached responses that the nodes share. They are outside the state, so a new response
    /// does not reload the state of the nodes.
    pub fn caches(&self) -> String {
        format!("{}cache/", self.data)
    }

    pub fn cache(&self, proxy: ShortId) -> String {
        format!("{}{proxy}/", self.caches())
    }

    /// The key holds the SHA-256 digest of the cache key, so a long URL fits in one key segment.
    pub fn cached_response(&self, proxy: ShortId, cache_key: &str) -> String {
        let digest = hex::encode(Sha256::digest(cache_key.as_bytes()));
        format!("{}{digest}", self.cache(proxy))
    }

    /// The Unix time in milliseconds of the last cache purge of each proxy. A change purges the
    /// cache of the proxy on every node.
    pub fn cache_purges(&self) -> String {
        format!("{}cache-purges/", self.state())
    }

    pub fn cache_purge(&self, proxy: ShortId) -> String {
        format!("{}{proxy}", self.cache_purges())
    }

    /// The audit log. It is outside the state, so a new entry does not reload the state of the
    /// nodes.
    pub fn audit(&self) -> String {
        format!("{}audit/", self.data)
    }

    /// The entries of a day in `YYYY-MM-DD` form.
    pub fn audit_day(&self, day: &str) -> String {
        format!("{}{day}/", self.audit())
    }

    /// The key of an entry. The time sorts the entries, and the random part keeps two entries of
    /// the same millisecond apart.
    pub fn audit_entry(&self, day: &str, time: u64, random: u32) -> String {
        format!("{}{time:013}-{random:08x}", self.audit_day(day))
    }

    /// The keys of the certificate events that the webhook got. Only the leader writes it. It is
    /// outside the state, so a change does not reload the state of the nodes.
    pub fn notifications(&self) -> String {
        format!("{}notify", self.data)
    }

    /// Whether the cluster stores the value of the key without encryption. Every other value is
    /// encrypted.
    pub fn is_plain(&self, key: &str) -> bool {
        let plain_prefixes = [
            self.ports(),
            self.challenges(),
            self.nodes(),
            self.acks(),
            self.cache_purges(),
        ];
        plain_prefixes.iter().any(|prefix| key.starts_with(prefix))
            || key == self.cdn()
            || key == self.schema()
    }

    /// The part of the state of a key. The keys outside the state have none.
    pub fn kind(&self, key: &str) -> Option<StateKind> {
        let rest = key.strip_prefix(&self.state())?;
        let part = rest.split('/').next().unwrap_or(rest);
        STATE_PARTS
            .iter()
            .find(|(name, _)| *name == part)
            .map(|(_, kind)| *kind)
    }
}

/// A key below `prefix` whose last segment holds the name in hex, so any name is one key segment.
fn hex_key(prefix: String, name: &str) -> String {
    format!("{prefix}{}", hex::encode(name))
}

/// The last segment of a key.
pub fn last_segment(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout::new("/r3v3rs3/")
    }

    fn id() -> ShortId {
        "web".parse().unwrap()
    }

    #[test]
    fn state_keys_are_below_the_versioned_prefix() {
        let layout = layout();
        assert_eq!(layout.config(), "r3v3rs3/v1/state/config");
        assert_eq!(layout.ports(), "r3v3rs3/v1/state/ports/");
        assert_eq!(
            layout.cert(CertKind::Server, id()),
            format!("r3v3rs3/v1/state/certs/{}/web", CertKind::Server)
        );
        assert_eq!(layout.account("a/b"), "r3v3rs3/v1/state/accounts/612f62");
        assert_eq!(last_segment(&layout.acme(id())), "web");
        assert_eq!(
            layout.access_list(id()),
            "r3v3rs3/v1/state/access-lists/web"
        );
        assert_eq!(layout.leader(), "r3v3rs3/v1/lock/leader");
    }

    #[test]
    fn state_keys_carry_their_part() {
        let layout = layout();
        assert_eq!(layout.kind(&layout.config()), Some(StateKind::Config));
        assert_eq!(
            layout.kind("r3v3rs3/v1/state/ports/web"),
            Some(StateKind::Ports)
        );
        assert_eq!(layout.kind(&layout.acme(id())), Some(StateKind::Acmes));
        assert_eq!(
            layout.kind(&layout.access_list(id())),
            Some(StateKind::AccessLists)
        );
        assert_eq!(
            layout.kind(&layout.account("admin")),
            Some(StateKind::Accounts)
        );
        assert_eq!(layout.kind("r3v3rs3/v1/state/unknown/web"), None);
        assert_eq!(layout.kind(&layout.leader()), None);
    }

    #[test]
    fn only_the_public_keys_are_plain() {
        let layout = layout();
        assert!(layout.is_plain("r3v3rs3/v1/state/ports/web"));
        assert!(layout.is_plain(&layout.cdn()));
        assert!(layout.is_plain(&layout.tls_alpn_challenge("example.com")));
        assert!(!layout.is_plain(&layout.config()));
        assert!(!layout.is_plain("r3v3rs3/v1/state/proxies/web"));
        assert!(!layout.is_plain(&layout.access_list(id())));
        assert!(!layout.is_plain(&layout.leader()));
        assert!(!layout.is_plain(&layout.account("admin")));
    }

    #[test]
    fn challenge_keys_hold_the_token_in_hex() {
        let layout = layout();
        let token = layout.http_challenge("a/b");
        assert_eq!(token, "r3v3rs3/v1/state/challenges/http/612f62");
        assert_eq!(layout.kind(&token), Some(StateKind::Challenges));
        assert!(layout.is_plain(&token));
    }

    #[test]
    fn node_keys_pair_with_their_ack() {
        let layout = layout();
        let node = layout.node("node-a");
        assert_eq!(node, "r3v3rs3/v1/nodes/6e6f64652d61");
        assert_eq!(layout.ack_of(&node), layout.ack("node-a"));
        assert!(layout.is_plain(&node));
        assert!(layout.is_plain(&layout.ack("node-a")));
        assert_eq!(layout.kind(&node), None);
    }

    #[test]
    fn session_and_rate_limit_keys_are_outside_the_state() {
        let layout = layout();
        let session = layout.session(SessionScope::Proxy, "ab12");
        assert_eq!(session, "r3v3rs3/v1/sessions/proxy/ab12");
        assert_eq!(layout.kind(&session), None);
        assert!(!layout.is_plain(&session));

        let counts = layout.rate_limit("node-a");
        assert_eq!(counts, "r3v3rs3/v1/ratelimit/6e6f64652d61");
        assert_eq!(layout.kind(&counts), None);
        assert!(!layout.is_plain(&counts));
    }

    #[test]
    fn cache_keys_hash_the_request() {
        let layout = layout();
        let response = layout.cached_response(id(), "localhost /");
        assert!(response.starts_with("r3v3rs3/v1/cache/web/"));
        assert_eq!(last_segment(&response).len(), 64);
        assert_eq!(layout.kind(&response), None);
        assert!(!layout.is_plain(&response));

        let purge = layout.cache_purge(id());
        assert_eq!(purge, "r3v3rs3/v1/state/cache-purges/web");
        assert_eq!(layout.kind(&purge), Some(StateKind::CachePurges));
        assert!(layout.is_plain(&purge));
    }

    #[test]
    fn audit_and_notify_keys_are_outside_the_state() {
        let layout = layout();
        let entry = layout.audit_entry("2026-09-15", 1, 255);
        assert_eq!(entry, "r3v3rs3/v1/audit/2026-09-15/0000000000001-000000ff");
        assert_eq!(layout.kind(&entry), None);
        assert!(!layout.is_plain(&entry));

        let sent = layout.notifications();
        assert_eq!(sent, "r3v3rs3/v1/notify");
        assert_eq!(layout.kind(&sent), None);
        assert!(!layout.is_plain(&sent));
    }
}
