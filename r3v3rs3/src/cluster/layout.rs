//! The keys of the cluster data below `{prefix}/v1/`.

use r3v3rs3_api::cert::CertKind;
use r3v3rs3_api::id::ShortId;

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
        assert!(!layout.is_plain(&layout.account("admin")));
    }
}
