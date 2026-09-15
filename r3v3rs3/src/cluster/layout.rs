//! The keys of the cluster data below `{prefix}/v1/`.

use crate::sessions::SessionScope;
use r3v3rs3_api::cert::CertKind;
use r3v3rs3_api::id::ShortId;

/// The parts of the state that a node reads again after a change in the store, in the order that
/// the node applies them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StateKind {
    Config,
    Certs,
    Acmes,
    Ports,
    Proxies,
    Cdn,
    Challenges,
}

impl StateKind {
    pub const ALL: [Self; 7] = [
        Self::Config,
        Self::Certs,
        Self::Acmes,
        Self::Ports,
        Self::Proxies,
        Self::Cdn,
        Self::Challenges,
    ];
}

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

    pub fn account(&self, name: &str) -> String {
        hex_key(format!("{}accounts/", self.state()), name)
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

    /// Whether the cluster stores the value of the key without encryption. Every other value is
    /// encrypted.
    pub fn is_plain(&self, key: &str) -> bool {
        let plain_prefixes = [self.ports(), self.challenges(), self.nodes(), self.acks()];
        plain_prefixes.iter().any(|prefix| key.starts_with(prefix))
            || key == self.cdn()
            || key == self.schema()
    }

    /// The part of the state of a key. Accounts and the keys outside the state have none.
    pub fn kind(&self, key: &str) -> Option<StateKind> {
        let rest = key.strip_prefix(&self.state())?;
        let part = rest.split('/').next().unwrap_or(rest);
        match part {
            "config" => Some(StateKind::Config),
            "certs" => Some(StateKind::Certs),
            "acme" => Some(StateKind::Acmes),
            "ports" => Some(StateKind::Ports),
            "proxies" => Some(StateKind::Proxies),
            "cdn" => Some(StateKind::Cdn),
            "challenges" => Some(StateKind::Challenges),
            _ => None,
        }
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

    #[test]
    fn keys_are_below_the_versioned_prefix() {
        let layout = Layout::new("/r3v3rs3/");
        let id = "web".parse::<ShortId>().unwrap();
        assert_eq!(layout.config(), "r3v3rs3/v1/state/config");
        assert_eq!(layout.ports(), "r3v3rs3/v1/state/ports/");
        assert_eq!(
            layout.cert(CertKind::Server, id),
            format!("r3v3rs3/v1/state/certs/{}/web", CertKind::Server)
        );
        assert_eq!(layout.account("a/b"), "r3v3rs3/v1/state/accounts/612f62");
        assert_eq!(last_segment(&layout.acme(id)), "web");

        assert!(layout.is_plain("r3v3rs3/v1/state/ports/web"));
        assert!(layout.is_plain(&layout.cdn()));
        assert!(!layout.is_plain(&layout.config()));
        assert!(!layout.is_plain("r3v3rs3/v1/state/proxies/web"));

        assert_eq!(layout.kind(&layout.config()), Some(StateKind::Config));
        assert_eq!(
            layout.kind("r3v3rs3/v1/state/ports/web"),
            Some(StateKind::Ports)
        );
        assert_eq!(layout.kind(&layout.acme(id)), Some(StateKind::Acmes));
        assert_eq!(layout.kind(&layout.account("admin")), None);
        assert_eq!(layout.leader(), "r3v3rs3/v1/lock/leader");
        assert_eq!(layout.kind(&layout.leader()), None);
        assert!(!layout.is_plain(&layout.leader()));

        let token = layout.http_challenge("a/b");
        assert_eq!(token, "r3v3rs3/v1/state/challenges/http/612f62");
        assert_eq!(layout.kind(&token), Some(StateKind::Challenges));
        assert!(layout.is_plain(&token));
        assert!(layout.is_plain(&layout.tls_alpn_challenge("example.com")));
        let node = layout.node("node-a");
        assert_eq!(node, "r3v3rs3/v1/nodes/6e6f64652d61");
        assert_eq!(layout.ack_of(&node), layout.ack("node-a"));
        assert!(layout.is_plain(&node));
        assert!(layout.is_plain(&layout.ack("node-a")));
        assert_eq!(layout.kind(&node), None);

        let session = layout.session(SessionScope::Proxy, "ab12");
        assert_eq!(session, "r3v3rs3/v1/sessions/proxy/ab12");
        assert_eq!(layout.kind(&session), None);
        assert!(!layout.is_plain(&session));
        assert!(!layout.is_plain(&layout.account("admin")));

        let counts = layout.rate_limit("node-a");
        assert_eq!(counts, "r3v3rs3/v1/ratelimit/6e6f64652d61");
        assert_eq!(layout.kind(&counts), None);
        assert!(!layout.is_plain(&counts));
    }
}
