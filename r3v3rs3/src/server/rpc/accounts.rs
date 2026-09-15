//! The panel accounts. Only an admin reads or changes them.

use super::RpcMethod;
use crate::accounts::Permission;
use crate::clock::unix_ms;
use crate::config::account::new_account;
use crate::server::state::ServerState;
use r3v3rs3_api::auth::{
    Account, AccountCreated, AccountInfo, AccountUpdate, NewAccount, Role, MIN_PASSWORD_LENGTH,
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use std::collections::{BTreeSet, HashMap};

/// The longest username. The username is a part of the admin API path of the account.
const MAX_USERNAME_LENGTH: usize = 64;

pub struct GetAccountList;

#[async_trait::async_trait]
impl RpcMethod for GetAccountList {
    type Output = Vec<AccountInfo>;
    const PERMISSION: Permission = Permission::Admin;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let accounts = state.storage.load_accounts().await?;
        let mut list = accounts
            .into_iter()
            .map(|(username, account)| AccountInfo {
                username,
                role: account.role,
                proxies: account.proxies,
                totp: account.totp.is_some(),
            })
            .collect::<Vec<_>>();
        list.sort_by(|a, b| a.username.cmp(&b.username));
        Ok(list)
    }
}

pub struct AddAccount {
    pub account: NewAccount,
}

#[async_trait::async_trait]
impl RpcMethod for AddAccount {
    type Output = AccountCreated;
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::Admin;

    /// The account starts with the current time as its last change, so a session of a removed
    /// account with the same username stays invalid.
    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let NewAccount {
            username,
            password,
            role,
            proxies,
            totp,
        } = self.account;
        validate_username(&username)?;
        validate_scope(role, proxies.as_ref())?;
        let mut account = hash_account(password, totp).await?;
        account.role = role;
        account.proxies = proxies;
        account.credentials_changed_at = unix_secs();
        let totp_secret = account.totp.clone();
        state
            .edit_accounts(move |accounts| {
                if accounts.contains_key(&username) {
                    return Err(Error::AccountExists { username });
                }
                accounts.insert(username, account);
                Ok(true)
            })
            .await?;
        Ok(AccountCreated { totp_secret })
    }
}

pub struct UpdateAccount {
    /// The account that sends the change.
    pub actor: String,
    pub username: String,
    pub update: AccountUpdate,
}

#[async_trait::async_trait]
impl RpcMethod for UpdateAccount {
    type Output = ();
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::Admin;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let AccountUpdate {
            role,
            proxies,
            password,
        } = self.update;
        if self.actor == self.username && !role.is_admin() {
            return Err(Error::CannotChangeOwnAccount);
        }
        validate_scope(role, proxies.as_ref())?;
        let password = match password {
            Some(password) => Some(hash_account(password, false).await?.password),
            None => None,
        };
        let username = self.username;
        state
            .edit_accounts(move |accounts| {
                let account =
                    accounts
                        .get_mut(&username)
                        .ok_or_else(|| Error::AccountNotFound {
                            username: username.clone(),
                        })?;
                apply_update(account, role, proxies, password);
                ensure_admin_remains(accounts)?;
                Ok(true)
            })
            .await
    }
}

pub struct DeleteAccount {
    /// The account that sends the change.
    pub actor: String,
    pub username: String,
}

#[async_trait::async_trait]
impl RpcMethod for DeleteAccount {
    type Output = ();
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::Admin;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        if self.actor == self.username {
            return Err(Error::CannotChangeOwnAccount);
        }
        let username = self.username;
        state
            .edit_accounts(move |accounts| {
                if accounts.remove(&username).is_none() {
                    return Err(Error::AccountNotFound { username });
                }
                ensure_admin_remains(accounts)?;
                Ok(true)
            })
            .await
    }
}

fn unix_secs() -> u64 {
    unix_ms() / 1000
}

fn validate_username(username: &str) -> Result<(), Error> {
    let invalid_char = |c: char| c == ':' || c == '/' || c.is_control() || c.is_whitespace();
    if username.is_empty()
        || username.chars().count() > MAX_USERNAME_LENGTH
        || username.chars().any(invalid_char)
    {
        return Err(Error::InvalidUsername {
            username: username.to_string(),
        });
    }
    Ok(())
}

/// An admin sees every proxy, so only the other roles take a proxy list.
fn validate_scope(role: Role, proxies: Option<&BTreeSet<ShortId>>) -> Result<(), Error> {
    if role.is_admin() && proxies.is_some() {
        return Err(Error::InvalidAccountScope);
    }
    Ok(())
}

/// Checks the password length and hashes the password on a blocking thread, because the hash
/// takes CPU time.
async fn hash_account(password: String, totp: bool) -> Result<Account, Error> {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(Error::PasswordTooShort {
            min: MIN_PASSWORD_LENGTH,
        });
    }
    tokio::task::spawn_blocking(move || new_account(&password, totp))
        .await
        .map_err(|_| Error::FailedToHashPassword)?
        .map_err(|_| Error::FailedToHashPassword)
}

/// Applies the change, and records the time of the change when the role, the proxy list or the
/// password changes.
fn apply_update(
    account: &mut Account,
    role: Role,
    proxies: Option<BTreeSet<ShortId>>,
    password: Option<String>,
) {
    let changed = account.role != role || account.proxies != proxies || password.is_some();
    account.role = role;
    account.proxies = proxies;
    if let Some(password) = password {
        account.password = password;
    }
    if changed {
        account.credentials_changed_at = unix_secs();
    }
}

fn ensure_admin_remains(accounts: &HashMap<String, Account>) -> Result<(), Error> {
    if accounts.values().any(|account| account.role.is_admin()) {
        Ok(())
    } else {
        Err(Error::LastAdmin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(role: Role) -> Account {
        Account {
            password: "$argon2id$hash".into(),
            role,
            ..Default::default()
        }
    }

    #[test]
    fn usernames_without_separators_are_valid() {
        assert!(validate_username("editor.1@example").is_ok());
        let long = "a".repeat(MAX_USERNAME_LENGTH + 1);
        for username in ["", "a:b", "a/b", "a b", "a\tb", long.as_str()] {
            assert!(
                matches!(
                    validate_username(username),
                    Err(Error::InvalidUsername { .. })
                ),
                "{username}"
            );
        }
    }

    #[test]
    fn only_a_role_without_admin_rights_takes_a_proxy_list() {
        let proxies = BTreeSet::from(["web".parse::<ShortId>().unwrap()]);
        assert!(validate_scope(Role::Editor, Some(&proxies)).is_ok());
        assert!(validate_scope(Role::Admin, None).is_ok());
        assert!(matches!(
            validate_scope(Role::Admin, Some(&proxies)),
            Err(Error::InvalidAccountScope)
        ));
    }

    #[test]
    fn at_least_one_admin_remains() {
        let mut accounts = HashMap::from([
            ("admin".to_string(), account(Role::Admin)),
            ("viewer".to_string(), account(Role::Viewer)),
        ]);
        assert!(ensure_admin_remains(&accounts).is_ok());
        accounts.remove("admin");
        assert!(matches!(
            ensure_admin_remains(&accounts),
            Err(Error::LastAdmin)
        ));
    }

    #[test]
    fn an_update_records_its_time_only_when_the_account_changes() {
        let mut viewer = account(Role::Viewer);
        apply_update(&mut viewer, Role::Viewer, None, None);
        assert_eq!(viewer.credentials_changed_at, 0);

        apply_update(
            &mut viewer,
            Role::Viewer,
            None,
            Some("$argon2id$new".into()),
        );
        assert_eq!(viewer.password, "$argon2id$new");
        assert!(viewer.credentials_changed_at > 0);

        let mut editor = account(Role::Editor);
        apply_update(&mut editor, Role::Viewer, None, None);
        assert_eq!(editor.role, Role::Viewer);
        assert!(editor.credentials_changed_at > 0);
    }

    #[tokio::test]
    async fn a_short_password_is_rejected_before_the_hash() {
        let result = hash_account("1234567".into(), false).await;
        assert!(matches!(
            result,
            Err(Error::PasswordTooShort {
                min: MIN_PASSWORD_LENGTH
            })
        ));
        let account = hash_account("12345678".into(), true).await.unwrap();
        assert!(account.password.starts_with("$argon2"));
        assert!(account.totp.is_some());
    }
}
