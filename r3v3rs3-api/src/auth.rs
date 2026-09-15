use crate::id::ShortId;
use serde_derive::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use utoipa::ToSchema;

/// What an account can change through the admin API.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Changes everything, including the settings and the accounts.
    #[default]
    Admin,
    /// Changes the proxies. Without a proxy list, also the ports, the certificates and the ACME
    /// entries.
    Editor,
    /// Reads the state and changes nothing.
    Viewer,
}

impl Role {
    pub fn is_admin(&self) -> bool {
        *self == Role::Admin
    }
}

/// The shortest password that a new account or a password change accepts.
pub const MIN_PASSWORD_LENGTH: usize = 8;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Account {
    pub password: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<String>,

    /// An account without a role is an admin.
    #[serde(default, skip_serializing_if = "Role::is_admin")]
    pub role: Role,

    /// The proxies that the account sees. `None` means every proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxies: Option<BTreeSet<ShortId>>,

    /// The Unix time in seconds of the last change of the role, the proxy list or the password. A
    /// session that started earlier is not valid.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub credentials_changed_at: u64,
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The password hash and the TOTP secret are secrets.
        f.debug_struct("Account")
            .field("password", &"***")
            .field("totp", &self.totp.as_ref().map(|_| "***"))
            .field("role", &self.role)
            .field("proxies", &self.proxies)
            .field("credentials_changed_at", &self.credentials_changed_at)
            .finish()
    }
}

/// An account without its secrets, which the admin API returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AccountInfo {
    #[schema(example = "admin")]
    pub username: String,
    pub role: Role,
    /// The proxies that the account sees. Absent means every proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<String>>)]
    pub proxies: Option<BTreeSet<ShortId>>,
    /// Whether the account signs in with a TOTP token too.
    pub totp: bool,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct NewAccount {
    #[schema(example = "editor")]
    pub username: String,
    #[schema(example = "passw0rd!")]
    pub password: String,
    #[serde(default)]
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<String>>)]
    pub proxies: Option<BTreeSet<ShortId>>,
    #[serde(default)]
    pub totp: bool,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct AccountUpdate {
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<String>>)]
    pub proxies: Option<BTreeSet<ShortId>>,
    /// A new password. Absent keeps the password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// The result of a new account. The admin API shows the TOTP secret only once.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct AccountCreated {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp_secret: Option<String>,
}

/// The account of the current admin session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct SessionInfo {
    #[schema(example = "admin")]
    pub username: String,
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<String>>)]
    pub proxies: Option<BTreeSet<ShortId>>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct LoginRequest {
    #[schema(example = "admin")]
    pub username: String,
    #[schema(inline)]
    #[serde(flatten)]
    pub method: LoginMethod,
    #[serde(default)]
    pub insecure: bool,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum LoginMethod {
    Password {
        #[schema(example = "passw0rd")]
        password: String,
    },
    Totp {
        #[schema(example = "234567")]
        token: String,
    },
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginResponse {
    Success,
    TotpRequired,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_without_a_role_is_an_admin_of_every_proxy() {
        let account: Account = serde_json::from_value(serde_json::json!({
            "password": "$argon2id$hash",
        }))
        .unwrap();
        assert_eq!(account.role, Role::Admin);
        assert_eq!(account.proxies, None);
        assert_eq!(account.credentials_changed_at, 0);
        assert!(!format!("{account:?}").contains("argon2"));

        // An admin account keeps the stored form of older versions.
        assert_eq!(
            serde_json::to_value(&account).unwrap(),
            serde_json::json!({"password": "$argon2id$hash"})
        );
    }

    #[test]
    fn a_restricted_editor_keeps_its_role_and_proxies() {
        let proxy: ShortId = "a1b2c3d4e5f6a7".parse().unwrap();
        let account = Account {
            password: "$argon2id$hash".into(),
            role: Role::Editor,
            proxies: Some(BTreeSet::from([proxy])),
            credentials_changed_at: 1_700_000_000,
            ..Default::default()
        };
        let value = serde_json::to_value(&account).unwrap();
        assert_eq!(value["role"], "editor");
        let parsed: Account = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.role, Role::Editor);
        assert_eq!(parsed.proxies, Some(BTreeSet::from([proxy])));
        assert_eq!(parsed.credentials_changed_at, 1_700_000_000);
    }

    #[test]
    fn an_unknown_role_is_rejected() {
        let result = serde_json::from_value::<Account>(serde_json::json!({
            "password": "$argon2id$hash",
            "role": "owner",
        }));
        assert!(result.is_err());
    }
}
