//! The accounts without their secrets. The admin API and the proxy authentication check them on
//! each request, so a change of a role or a proxy list applies to the sessions that already
//! started.

use crate::sessions::SessionRecord;
use r3v3rs3_api::auth::{Account, Role};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::id::ShortId;
use std::collections::{BTreeSet, HashMap};

/// What an RPC method needs from the account that calls it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Read,
    /// Changes the proxies. An editor with a proxy list changes only the proxies of its list.
    EditProxies,
    /// Changes the ports, the certificates and the ACME entries.
    Edit,
    /// Reads or changes the settings, and runs the internal methods of the admin API.
    Admin,
}

/// The account of an admin API request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub username: String,
    pub role: Role,
    /// The proxies that the account sees. `None` means every proxy.
    pub proxies: Option<BTreeSet<ShortId>>,
}

impl Caller {
    /// The admin API itself, for example before the sign-in of an account.
    pub fn system() -> Self {
        Self {
            username: String::new(),
            role: Role::Admin,
            proxies: None,
        }
    }

    pub fn authorize(&self, permission: Permission) -> Result<(), Error> {
        if self.allows(permission) {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }

    pub fn can_see(&self, proxy: ShortId) -> bool {
        self.proxies
            .as_ref()
            .is_none_or(|proxies| proxies.contains(&proxy))
    }

    /// A proxy that the account does not see does not exist for the account.
    pub fn ensure_visible(&self, proxy: ShortId) -> Result<(), Error> {
        if self.can_see(proxy) {
            Ok(())
        } else {
            Err(Error::IdNotFound {
                id: proxy.to_string(),
            })
        }
    }

    /// The part of a server event that the account sees. Only an admin gets the settings.
    pub fn visible_event(&self, event: ServerEvent) -> Option<ServerEvent> {
        match event {
            ServerEvent::AppConfigUpdated { .. } if !self.role.is_admin() => None,
            ServerEvent::ProxiesUpdated { mut entries } => {
                entries.retain(|entry| self.can_see(entry.id));
                Some(ServerEvent::ProxiesUpdated { entries })
            }
            ServerEvent::ProxyStatusUpdated { id, .. } if !self.can_see(id) => None,
            event => Some(event),
        }
    }

    fn allows(&self, permission: Permission) -> bool {
        match permission {
            Permission::Read => true,
            Permission::EditProxies => self.role != Role::Viewer,
            Permission::Edit => {
                self.role.is_admin() || (self.role == Role::Editor && self.proxies.is_none())
            }
            Permission::Admin => self.role.is_admin(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountEntry {
    pub role: Role,
    /// The proxies that the account sees. `None` means every proxy.
    pub proxies: Option<BTreeSet<ShortId>>,
    pub credentials_changed_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountDirectory {
    accounts: HashMap<String, AccountEntry>,
}

impl AccountDirectory {
    pub fn new(accounts: &HashMap<String, Account>) -> Self {
        let accounts = accounts
            .iter()
            .map(|(name, account)| {
                let entry = AccountEntry {
                    role: account.role,
                    proxies: account.proxies.clone(),
                    credentials_changed_at: account.credentials_changed_at,
                };
                (name.clone(), entry)
            })
            .collect();
        Self { accounts }
    }

    pub fn get(&self, username: &str) -> Option<&AccountEntry> {
        self.accounts.get(username)
    }

    /// The caller of an admin session. A session of a removed account, or a session that started
    /// before the last change of the account, has none.
    pub fn caller(&self, session: &SessionRecord) -> Option<Caller> {
        let entry = self.get(&session.subject)?;
        (session.started_at >= entry.credentials_changed_at).then(|| Caller {
            username: session.subject.clone(),
            role: entry.role,
            proxies: entry.proxies.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_keeps_the_role_and_the_proxies_of_each_account() {
        let proxies = Some(BTreeSet::from(["web".parse::<ShortId>().unwrap()]));
        let accounts = HashMap::from([(
            "editor".to_string(),
            Account {
                password: "$argon2id$hash".into(),
                role: Role::Editor,
                proxies: proxies.clone(),
                credentials_changed_at: 7,
                ..Default::default()
            },
        )]);
        let directory = AccountDirectory::new(&accounts);
        let expected = AccountEntry {
            role: Role::Editor,
            proxies,
            credentials_changed_at: 7,
        };
        assert_eq!(directory.get("editor"), Some(&expected));
        assert_eq!(directory.get("admin"), None);
    }

    #[test]
    fn a_session_before_the_last_account_change_has_no_caller() {
        let accounts = HashMap::from([(
            "viewer".to_string(),
            Account {
                role: Role::Viewer,
                credentials_changed_at: 100,
                ..Default::default()
            },
        )]);
        let directory = AccountDirectory::new(&accounts);
        let session = |subject: &str, started_at| SessionRecord {
            subject: subject.to_string(),
            started_at,
        };
        let expected = Caller {
            username: "viewer".to_string(),
            role: Role::Viewer,
            proxies: None,
        };
        assert_eq!(directory.caller(&session("viewer", 100)), Some(expected));
        assert_eq!(directory.caller(&session("viewer", 99)), None);
        assert_eq!(directory.caller(&session("removed", 100)), None);
    }

    #[test]
    fn an_account_with_a_proxy_list_gets_only_the_events_of_its_proxies() {
        let web = "web".parse::<ShortId>().unwrap();
        let other = "other".parse::<ShortId>().unwrap();
        let entry = |id| r3v3rs3_api::proxy::ProxyEntry {
            id,
            proxy: Default::default(),
            source: None,
        };
        let viewer = Caller {
            username: "viewer".to_string(),
            role: Role::Viewer,
            proxies: Some(BTreeSet::from([web])),
        };
        let config = ServerEvent::AppConfigUpdated {
            config: Default::default(),
        };
        assert!(viewer.visible_event(config.clone()).is_none());
        assert!(Caller::system().visible_event(config).is_some());

        let proxies = ServerEvent::ProxiesUpdated {
            entries: vec![entry(web), entry(other)],
        };
        let Some(ServerEvent::ProxiesUpdated { entries }) = viewer.visible_event(proxies) else {
            panic!("the proxy list event is missing");
        };
        assert_eq!(entries, vec![entry(web)]);
        assert!(viewer.ensure_visible(web).is_ok());
        assert!(matches!(
            viewer.ensure_visible(other),
            Err(Error::IdNotFound { .. })
        ));
    }

    #[test]
    fn each_role_gets_its_permissions() {
        let caller = |role, proxies: Option<BTreeSet<ShortId>>| Caller {
            username: "user".to_string(),
            role,
            proxies,
        };
        let restricted = Some(BTreeSet::from(["web".parse::<ShortId>().unwrap()]));
        let cases = [
            (caller(Role::Admin, None), [true, true, true, true]),
            (caller(Role::Editor, None), [true, true, true, false]),
            (
                caller(Role::Editor, restricted.clone()),
                [true, true, false, false],
            ),
            (
                caller(Role::Viewer, restricted),
                [true, false, false, false],
            ),
        ];
        let permissions = [
            Permission::Read,
            Permission::EditProxies,
            Permission::Edit,
            Permission::Admin,
        ];
        for (caller, allowed) in cases {
            for (permission, allowed) in permissions.into_iter().zip(allowed) {
                let result = caller.authorize(permission);
                let expected = if allowed {
                    result.is_ok()
                } else {
                    matches!(result, Err(Error::Forbidden))
                };
                assert!(expected, "{caller:?} {permission:?}");
            }
        }
    }
}
