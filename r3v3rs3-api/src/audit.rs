//! The audit log: the changes that the accounts make through the admin API, and the sign-ins of
//! the admin panel.

use serde_derive::{Deserialize, Serialize};
use std::net::IpAddr;
use utoipa::{IntoParams, ToSchema};

/// The longest summary of an entry in bytes.
pub const MAX_SUMMARY_LENGTH: usize = 512;

/// The entries of a query without `limit`.
pub const DEFAULT_QUERY_LIMIT: u32 = 100;

/// The most entries of a query.
pub const MAX_QUERY_LIMIT: u32 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    Login,
    LoginFailed,
    Logout,
    AddAccount,
    UpdateAccount,
    DeleteAccount,
    UpdateConfig,
    RefreshCdnRanges,
    AddPort,
    UpdatePort,
    DeletePort,
    ResetPort,
    AddProxy,
    UpdateProxy,
    DeleteProxy,
    PurgeProxyCache,
    AddCert,
    DeleteCert,
    AddAcme,
    UpdateAcme,
    DeleteAcme,
}

impl AuditAction {
    pub const ALL: [AuditAction; 21] = [
        Self::Login,
        Self::LoginFailed,
        Self::Logout,
        Self::AddAccount,
        Self::UpdateAccount,
        Self::DeleteAccount,
        Self::UpdateConfig,
        Self::RefreshCdnRanges,
        Self::AddPort,
        Self::UpdatePort,
        Self::DeletePort,
        Self::ResetPort,
        Self::AddProxy,
        Self::UpdateProxy,
        Self::DeleteProxy,
        Self::PurgeProxyCache,
        Self::AddCert,
        Self::DeleteCert,
        Self::AddAcme,
        Self::UpdateAcme,
        Self::DeleteAcme,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AuditEntry {
    /// The Unix time in milliseconds.
    pub time: u64,
    /// The account. A failed sign-in has the typed username.
    pub username: String,
    /// The IP address of the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "192.0.2.10")]
    pub client: Option<IpAddr>,
    pub action: AuditAction,
    /// The id of the changed port, proxy, certificate or ACME entry, or the changed username.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    /// The names, the addresses and the roles of the change. It holds no password, token or key.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// The cluster node that recorded the entry. Empty without a cluster.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node: String,
}

/// The filter of an audit log query. Each field is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditQuery {
    /// The earliest time in Unix milliseconds. The default is 31 days before `until`.
    pub since: Option<u64>,
    /// The latest time in Unix milliseconds. The default is the current time.
    pub until: Option<u64>,
    /// The account of the entries.
    pub username: Option<String>,
    /// The id of the changed resource, or the changed username.
    pub resource_id: Option<String>,
    /// The most entries in the response: 100 by default, at most 500.
    pub limit: Option<u32>,
}
