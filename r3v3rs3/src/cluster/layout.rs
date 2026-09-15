//! The keys of the cluster data below `{prefix}/v1/`.

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
}

impl StateKind {
    pub const ALL: [Self; 6] = [
        Self::Config,
        Self::Certs,
        Self::Acmes,
        Self::Ports,
        Self::Proxies,
        Self::Cdn,
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

    /// The account key holds the user name in hex, so any name is one key segment.
    pub fn account(&self, name: &str) -> String {
        format!("{}accounts/{}", self.state(), hex::encode(name))
    }

    pub fn cdn(&self) -> String {
        format!("{}cdn", self.state())
    }

    /// Whether the cluster stores the value of the key without encryption. Every other value is
    /// encrypted.
    pub fn is_plain(&self, key: &str) -> bool {
        key.starts_with(&self.ports()) || key == self.cdn() || key == self.schema()
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
            _ => None,
        }
    }
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
        assert_eq!(layout.kind("r3v3rs3/v1/lock/leader"), None);
        assert!(!layout.is_plain(&layout.account("admin")));
    }
}
