//! The accounts without their secrets. The admin API and the proxy authentication check them on
//! each request, so a change of a role or a proxy list applies to the sessions that already
//! started.

use r3v3rs3_api::auth::{Account, Role};
use r3v3rs3_api::id::ShortId;
use std::collections::{BTreeSet, HashMap};

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
}
