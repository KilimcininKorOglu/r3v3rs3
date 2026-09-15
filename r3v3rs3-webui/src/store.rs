use r3v3rs3_api::{
    acme::AcmeInfo,
    auth::{Role, SessionInfo},
    cert::CertInfo,
    cluster::ClusterStatus,
    discovery::DiscoveryStatus,
    id::ShortId,
    port::{PortEntry, PortStatus},
    proxy::{ProxyEntry, ProxyStatus},
};
use std::collections::HashMap;
use yewdux::prelude::*;

#[derive(Default, Clone, PartialEq, Store)]
pub struct PortStore {
    pub entries: Vec<PortEntry>,
    pub statuses: HashMap<ShortId, PortStatus>,
    pub loaded: bool,
}

#[derive(Default, Clone, PartialEq, Store)]
pub struct ProxyStore {
    pub entries: Vec<ProxyEntry>,
    pub statuses: HashMap<ShortId, ProxyStatus>,
    pub loaded: bool,
}

#[derive(Default, Clone, PartialEq, Store)]
pub struct CertStore {
    pub entries: Vec<CertInfo>,
    pub loaded: bool,
}

#[derive(Default, Clone, PartialEq, Store)]
pub struct AcmeStore {
    pub entries: Vec<AcmeInfo>,
    pub loaded: bool,
}

#[derive(Default, Clone, PartialEq, Store)]
pub struct DiscoveryStore {
    pub entries: Vec<DiscoveryStatus>,
}

#[derive(Default, Clone, PartialEq, Store)]
pub struct ClusterStore {
    pub status: ClusterStatus,
}

/// The account of the admin session. The WebUI hides the actions that the role of the account
/// does not allow, and the admin API rejects them too. Before the session loads, no action is
/// allowed.
#[derive(Default, Clone, PartialEq, Store)]
pub struct SessionStore {
    pub info: Option<SessionInfo>,
}

impl SessionStore {
    pub fn is_admin(&self) -> bool {
        self.info.as_ref().is_some_and(|info| info.role.is_admin())
    }

    /// Whether the account changes the proxies that it sees.
    pub fn can_edit_proxies(&self) -> bool {
        self.info
            .as_ref()
            .is_some_and(|info| info.role != Role::Viewer)
    }

    /// Whether the account changes the ports, the certificates and the ACME entries.
    pub fn can_edit(&self) -> bool {
        self.info.as_ref().is_some_and(|info| {
            info.role.is_admin() || (info.role == Role::Editor && info.proxies.is_none())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn each_role_gets_the_actions_of_the_admin_api() {
        let session = |role, proxies: Option<BTreeSet<ShortId>>| SessionStore {
            info: Some(SessionInfo {
                username: "user".into(),
                role,
                proxies,
                cert_expiry_warning: Default::default(),
            }),
        };
        let restricted = Some(BTreeSet::from(["web".parse().unwrap()]));
        let cases = [
            (SessionStore::default(), [false, false, false]),
            (session(Role::Admin, None), [true, true, true]),
            (session(Role::Editor, None), [false, true, true]),
            (
                session(Role::Editor, restricted.clone()),
                [false, true, false],
            ),
            (session(Role::Viewer, restricted), [false, false, false]),
        ];
        for (session, [admin, proxies, edit]) in cases {
            assert_eq!(session.is_admin(), admin, "{:?}", session.info);
            assert_eq!(session.can_edit_proxies(), proxies, "{:?}", session.info);
            assert_eq!(session.can_edit(), edit, "{:?}", session.info);
        }
    }
}
