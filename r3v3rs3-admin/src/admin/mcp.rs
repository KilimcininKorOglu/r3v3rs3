//! The Model Context Protocol server of the admin API.
//!
//! Every tool runs through the same RPC methods that the admin API handlers use, so the
//! permission checks, the proxy scopes and the audit log of the server apply unchanged. The
//! tools answer with pretty-printed JSON, because the schemas of the API types are larger than
//! a tool argument needs to be.

use super::{AppState, McpScope};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::server::rpc::access_lists::{
    AddAccessList, DeleteAccessList, GetAccessLists, UpdateAccessList,
};
use crate::server::rpc::accounts::{
    AddAccount, CreateApiToken, DeleteAccount, DeleteApiToken, GetAccountList, UpdateAccount,
};
use crate::server::rpc::acme::{
    AddAcme, DeleteAcme, GetAcme, GetAcmeCnameInstructions, GetAcmeList, RequestAcme,
};
use crate::server::rpc::auth::GetAuditLog;
use crate::server::rpc::certs::{DeleteCert, GetCert, GetCertCoverage, GetCertList};
use crate::server::rpc::config::GetConfig;
use crate::server::rpc::dns_credential::GetDnsCredentialList;
use crate::server::rpc::ports::{
    AddPort, DeletePort, GetNetworkInterfaceList, GetPort, GetPortList, GetPortStatus, ResetPort,
    UpdatePort,
};
use crate::server::rpc::proxies::{
    AddProxy, ApplyProxySecurityHeadersProfile, DeleteProxy, GetProxy, GetProxyList,
    GetProxyStatus, PurgeProxyCache, StreamPriorityMutation, UpdateProxy,
    UpdateProxyStreamPriority,
};
use crate::server::rpc::workers::GetWorkers;
use http::request::Parts;
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::compression_dictionary::CompressionDictionary;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry, ProxyKind};
use rmcp::ErrorData;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{tool, tool_handler, tool_router};
use serde_json::json;

pub use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
pub use rmcp::transport::streamable_http_server::tower::{
    StreamableHttpServerConfig, StreamableHttpService,
};

/// The tools that the MCP endpoint serves.
#[derive(Clone)]
pub struct McpServer {
    state: AppState,
    tool_router: rmcp::handler::server::router::tool::ToolRouter<McpServer>,
}

/// The `{ "id": "..." }` argument of the tools that address one resource.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IdParams {
    /// The id of the resource (port, proxy, certificate, app, etc.).
    pub id: String,
}

/// The `{ "ip": "..." }` argument of the tools that address an IP address.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IpParams {
    /// The IP address.
    pub ip: String,
}

/// Arguments for querying the audit log.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AuditLogParams {
    /// Maximum number of audit log entries to return (default: 50).
    pub limit: Option<u32>,
}

/// Arguments for listing active long-lived connections.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListActiveConnectionsParams {
    /// Optional proxy ID filter.
    pub proxy_id: Option<String>,
}

/// The `{ "port": {...} }` argument of `create_port`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddPortParams {
    /// The port configuration, in the same form as the body of POST /api/ports.
    pub port: r3v3rs3_api::port::Port,
}

/// The `{ "entry": {...} }` argument of `update_port`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdatePortParams {
    /// The port entry with its id and new configuration.
    pub entry: r3v3rs3_api::port::PortEntry,
}

/// The argument of `update_port_early_data`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetPortEarlyDataParams {
    /// The id of the port.
    pub id: String,
    /// Whether TLS 1.3 0-RTT early data should be enabled.
    pub enabled: bool,
}

/// The argument of `update_port_transparent`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetPortTransparentParams {
    /// The id of the port.
    pub id: String,
    /// Whether the upstream servers see the client address.
    pub enabled: bool,
}

/// The argument of `restore_config`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RestoreConfigParams {
    /// The backup object, as `backup_config` returns it.
    pub backup: serde_json::Map<String, serde_json::Value>,
}

/// A request body whose API type has no JSON schema; the tool checks it against the type.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct JsonRequestParams {
    /// The request body, as the matching REST endpoint takes it.
    pub request: serde_json::Map<String, serde_json::Value>,
}

/// The id of a resource and a request body whose API type has no JSON schema.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IdJsonRequestParams {
    /// The id of the resource.
    pub id: String,
    /// The request body, as the matching REST endpoint takes it.
    pub request: serde_json::Map<String, serde_json::Value>,
}

/// The arguments of `get_logs`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct LogsParams {
    /// `server`, or a port or proxy id.
    pub id: String,
    /// The oldest row, as Unix seconds.
    pub since: Option<i64>,
    /// The newest row, as Unix seconds.
    pub until: Option<i64>,
    /// The most rows: 100 by default, at most 500.
    pub limit: Option<u32>,
}

/// The filters of `get_traffic` and `get_traffic_geo`.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct TrafficQueryParams {
    /// Only entries of this proxy.
    pub resource_id: Option<String>,
    /// Only entries with this HTTP status.
    pub status: Option<u16>,
    /// Only entries whose host, path or client holds this text.
    pub search: Option<String>,
    /// The most entries to return.
    pub limit: Option<usize>,
    /// Only entries newer than this entry id.
    pub since_id: Option<u64>,
    /// Only entries from this ISO country code.
    pub country: Option<String>,
}

impl TrafficQueryParams {
    fn query(self) -> Result<r3v3rs3_api::traffic::AccessLogQuery, ErrorData> {
        let resource_id = self.resource_id.as_deref().map(parse_id).transpose()?;
        Ok(r3v3rs3_api::traffic::AccessLogQuery {
            resource_id,
            status: self.status,
            search: self.search,
            limit: self.limit,
            since_id: self.since_id,
            country: self.country,
        })
    }
}

/// The arguments of `get_traffic_entry`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TrafficEntryParams {
    /// The id of the access log entry.
    pub id: u64,
}

/// The arguments of `upload_cert`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UploadCertParams {
    /// The PEM certificate chain.
    pub chain: String,
    /// The optional PEM private key.
    pub key: Option<String>,
    /// `server` (default), `client` or `root`.
    pub kind: Option<String>,
}

/// The arguments of `verify_cname`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct VerifyCnameParams {
    /// The domain whose CNAME record is checked.
    pub domain: String,
    /// The host the CNAME must point at; default: the host of the public URL.
    pub expected_target: Option<String>,
}

/// The arguments of `update_project`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateProjectParams {
    /// The id of the project.
    pub id: String,
    /// The new name, description and default environments.
    pub spec: r3v3rs3_api::platform::ProjectSpec,
}

/// A project and one of its environments.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ProjectEnvironmentParams {
    /// The id of the project.
    pub project_id: String,
    /// The id of the environment.
    pub environment_id: String,
}

impl ProjectEnvironmentParams {
    fn ids(&self) -> Result<(ShortId, ShortId), ErrorData> {
        Ok((parse_id(&self.project_id)?, parse_id(&self.environment_id)?))
    }
}

/// The arguments of `update_environment`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateEnvironmentParams {
    /// The id of the project.
    pub project_id: String,
    /// The id of the environment.
    pub environment_id: String,
    /// The new `name` and optional `description`.
    pub request: serde_json::Map<String, serde_json::Value>,
}

/// The arguments of `diff_environments`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DiffEnvironmentsParams {
    /// The id of the project.
    pub project_id: String,
    /// The environment compared from.
    pub source_id: String,
    /// The environment compared to.
    pub target_id: String,
}

/// The arguments of `update_env_group`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateEnvGroupParams {
    /// The id of the group.
    pub id: String,
    /// The new name, description and variables.
    pub group: r3v3rs3_api::platform::EnvGroupSpec,
}

/// The arguments of `get_app_metrics`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AppMetricsParams {
    /// The id of the app.
    pub id: String,
    /// How far back the history goes, from 1 to 24 hours. The default is 6.
    pub hours: Option<u32>,
}

/// The arguments of `update_app_git_token`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct GitTokenParams {
    /// The id of the app.
    pub id: String,
    /// The access token that clones the private repository.
    pub token: String,
}

/// The token stays out of debug output and logs.
impl std::fmt::Debug for GitTokenParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitTokenParams")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// The optional filter of `list_s3_database_backups`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct S3ListParams {
    /// Only the snapshots of this database name.
    pub database: Option<String>,
}

/// Turns a JSON object argument into its API type; a mismatch is an invalid argument.
fn json_arg<T: serde::de::DeserializeOwned>(
    object: serde_json::Map<String, serde_json::Value>,
) -> Result<T, ErrorData> {
    serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|err| ErrorData::invalid_params(err.to_string(), None))
}

/// The argument of `create_dns_credential`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DnsCredentialParams {
    /// The body of POST /api/dns_credentials.
    pub request: r3v3rs3_api::acme::DnsCredentialRequest,
}

/// The arguments of `update_dns_credential`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateDnsCredentialParams {
    /// The id of the DNS credential.
    pub id: String,
    /// The new name and, optionally, new secrets.
    pub request: r3v3rs3_api::acme::DnsCredentialRequest,
}

/// The argument of `update_firewall`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetFirewallParams {
    /// Whether r3v3rs3 keeps the host firewall table.
    pub enabled: Option<bool>,
    /// The extra ports the table opens, such as `udp/51820` or `tcp/8000-8010`; replaces the list.
    pub extra_ports: Option<Vec<String>>,
}

/// Applies the given fields of `update_firewall`; an invalid port is refused before anything changes.
fn apply_firewall_params(
    firewall: &mut r3v3rs3_api::firewall::FirewallConfig,
    params: SetFirewallParams,
) -> Result<(), ErrorData> {
    if let Some(ports) = params.extra_ports {
        firewall.extra_ports = ports
            .iter()
            .map(|port| port.trim().parse())
            .collect::<Result<_, _>>()
            .map_err(|err| ErrorData::invalid_params(format!("{err}"), None))?;
    }
    if let Some(enabled) = params.enabled {
        firewall.enabled = enabled;
    }
    Ok(())
}

/// The argument of `update_ct_config`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateCtConfigParams {
    /// Whether CT monitoring is enabled.
    pub enabled: Option<bool>,
    /// Check interval in seconds (default: 21600).
    pub check_interval_secs: Option<u64>,
    /// Additional domains to monitor.
    pub monitored_domains: Option<Vec<String>>,
    /// Auto-discover domains from server certs and ACME orders.
    pub auto_discover_domains: Option<bool>,
    /// List of trusted CA issuer substrings.
    pub allowed_issuers: Option<Vec<String>>,
    /// Whether to flag unauthorized issuers as rogue.
    pub alert_on_unknown_issuer: Option<bool>,
    /// Custom CT API template URL.
    pub ct_api_url: Option<String>,
}

/// The argument of `list_ct_entries`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListCtEntriesParams {
    /// Optional domain filter.
    pub domain: Option<String>,
    /// Only return rogue or suspicious entries.
    pub rogue_only: Option<bool>,
    /// Maximum number of records (default: 50).
    pub limit: Option<usize>,
}

/// The argument of `check_ct_now`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CheckCtParams {
    /// Specific domain to check. If omitted, checks all configured and discovered domains.
    pub domain: Option<String>,
}

/// The argument of `dismiss_ct_alert`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DismissCtAlertParams {
    /// Unique entry ID.
    pub id: String,
    /// Optional status: "investigating", "dismissed", or "confirmed_rogue" (default: "dismissed").
    pub status: Option<String>,
}

/// The `{ "proxy": {...} }` argument of `create_proxy`, in the form of the admin API.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddProxyParams {
    /// The proxy configuration, in the same form as the body of POST /api/proxies.
    pub proxy: r3v3rs3_api::proxy::Proxy,
}

/// The `{ "entry": {...} }` argument of `update_proxy`, in the form of the admin API.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateProxyParams {
    /// The id and the new configuration of the proxy, in the same form as the body of
    /// PUT /api/proxies/{id}.
    pub entry: r3v3rs3_api::proxy::ProxyEntry,
}

/// The argument of `purge_proxy_cache`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PurgeProxyCacheParams {
    /// The id of the proxy.
    pub id: String,
    /// Optional URI path prefix to purge. Empty purges the entire cache.
    pub path: Option<String>,
}

/// The argument of `update_proxy_stream_priority`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetStreamPriorityParams {
    /// The id of the proxy.
    pub id: String,
    /// Whether adaptive stream priority should be enabled.
    pub enabled: bool,
    /// Whether to inject RFC 9218 Priority headers upstream.
    pub inject_header: Option<bool>,
    /// Whether to apply adaptive concurrency backpressure.
    pub adaptive_concurrency: Option<bool>,
}

/// The argument of `check_cert_coverage`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CertCoverageParams {
    /// List of domain names to verify certificate coverage for.
    pub domains: Vec<String>,
}

/// The `{ "request": {...} }` argument of `create_acme`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddAcmeParams {
    /// The ACME certificate request configuration.
    pub request: r3v3rs3_api::acme::AcmeRequest,
}

/// The `{ "list": {...} }` argument of `create_access_list`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddAccessListParams {
    /// The access control list configuration.
    pub list: r3v3rs3_api::access_list::AccessList,
}

/// The `{ "entry": {...} }` argument of `update_access_list`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateAccessListParams {
    /// The access list entry with its id and configuration.
    pub entry: r3v3rs3_api::access_list::AccessListEntry,
}

/// The `{ "app": {...} }` argument of `create_app`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddAppParams {
    /// The application specification, in the same form as the body of POST /api/apps.
    pub app: r3v3rs3_api::platform::AppRequest,
}

/// The `{ "id": "...", "app": {...} }` argument of `update_app`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateAppParams {
    /// The id of the application.
    pub id: String,
    /// The updated application specification.
    pub app: r3v3rs3_api::platform::AppRequest,
}

/// The arguments of `get_app_logs`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AppLogsParams {
    /// The id of the application.
    pub id: String,
    /// Number of lines to return from the end of the container log, 1 to 1000 (default: 200).
    pub tail: Option<u32>,
    /// Optional service name for multi-service Compose applications.
    pub service: Option<String>,
}

/// The `{ "id": "...", "entries": [...] }` argument of `update_app_env`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateAppEnvParams {
    /// The id of the application.
    pub id: String,
    /// The list of environment variables (`[{"key": "FOO", "value": "bar", "secret": false}]`).
    pub entries: Vec<r3v3rs3_api::platform::EnvEntry>,
}

/// The arguments of `generate_app_env_secrets`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GenerateAppEnvSecretsParams {
    /// The id of the application.
    pub id: String,
    /// Whether to save the generated secrets to the application (default: true).
    pub save: Option<bool>,
    /// Whether to overwrite existing non-empty values (default: false).
    pub overwrite: Option<bool>,
}

/// The arguments of `resolve_compose_env_template`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ResolveComposeEnvParams {
    /// Optional raw .env template string.
    pub template: Option<String>,
    /// Optional list of environment entries (`[{"key": "FOO", "value": "bar", "secret": true}]`).
    pub entries: Option<Vec<r3v3rs3_api::platform::EnvEntry>>,
    /// Whether to overwrite existing non-empty values (default: false).
    pub overwrite: Option<bool>,
}

/// The `{ "deployment_id": "..." }` argument of `rollback_deployment`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RollbackParams {
    /// The deployment id to roll back to.
    pub deployment_id: String,
}

/// The arguments of `create_app_scheduled_command`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddAppScheduledCommandParams {
    /// The id of the application.
    pub app_id: String,
    /// Human-friendly name for this scheduled command.
    pub name: String,
    /// Cron expression (e.g. "0 3 * * *", "@hourly", "@daily").
    pub schedule: String,
    /// Shell command string to execute in the container.
    pub command: String,
    /// Whether the scheduled command is active (default: true).
    pub enabled: Option<bool>,
}

/// The arguments of `update_app_scheduled_command`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateAppScheduledCommandParams {
    /// The id of the application.
    pub app_id: String,
    /// The id of the scheduled command.
    pub command_id: String,
    /// Human-friendly name for this scheduled command.
    pub name: String,
    /// Cron expression (e.g. "0 3 * * *", "@hourly", "@daily").
    pub schedule: String,
    /// Shell command string to execute in the container.
    pub command: String,
    /// Whether the scheduled command is active.
    pub enabled: Option<bool>,
}

/// The arguments of `delete_app_scheduled_command` and `run_app_scheduled_command`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AppCommandTargetParams {
    /// The id of the application.
    pub app_id: String,
    /// The id of the scheduled command.
    pub command_id: String,
}

/// The arguments of `create_outbound_webhook`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddWebhookParams {
    /// Name of the webhook.
    pub name: String,
    /// HTTPS delivery endpoint URL.
    pub url: String,
    /// Optional shared secret for HMAC-SHA256 signature verification.
    pub secret: Option<String>,
    /// Whether the webhook is enabled (default: true).
    pub enabled: Option<bool>,
    /// List of event names (e.g. ["deploy.success", "backup.failure", "security.ip_banned"]).
    pub events: Vec<String>,
}

/// The arguments of `update_outbound_webhook`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateWebhookParams {
    /// Webhook ID to update.
    pub id: String,
    /// Optional updated name.
    pub name: Option<String>,
    /// Optional updated HTTPS URL.
    pub url: Option<String>,
    /// Optional updated secret (empty string clears secret).
    pub secret: Option<String>,
    /// Optional updated enabled flag.
    pub enabled: Option<bool>,
    /// Optional updated list of event subscriptions.
    pub events: Option<Vec<String>>,
}

/// The arguments of `test_outbound_webhook`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TestWebhookParams {
    /// Optional ID of an existing webhook to test.
    pub webhook_id: Option<String>,
    /// Optional direct URL to test (used when testing before saving).
    pub url: Option<String>,
    /// Optional secret for signing the test payload.
    pub secret: Option<String>,
    /// Optional simulated event type (default: test.ping).
    pub event: Option<String>,
}

/// The arguments of `create_registry`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddRegistryParams {
    /// Container registry server address (e.g. "docker.io", "ghcr.io", "registry.gitlab.com").
    pub server: String,
    /// Username or service account for authentication.
    pub username: String,
    /// Password or personal access token with pull permissions.
    pub password: String,
}

/// The arguments of `update_registry`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateRegistryParams {
    /// Registry ID to update.
    pub id: String,
    /// Container registry server address (e.g. "docker.io", "ghcr.io").
    pub server: String,
    /// Username or service account for authentication.
    pub username: String,
    /// Optional updated password or token (empty string or omitted keeps current).
    pub password: Option<String>,
}

/// The arguments of `create_git_connection`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddGitConnectionParams {
    /// Distinct identifier name for the connection (e.g. "github-org", "company-gitlab").
    pub name: String,
    /// Git provider type: "gitlab" or "gitea". A GitHub connection is created as a GitHub App in the panel.
    pub provider: String,
    /// Base URL of a self-hosted provider (e.g. "https://gitlab.example.com"). Required for Gitea; GitLab defaults to https://gitlab.com.
    pub url: Option<String>,
    /// Client ID of the OAuth application.
    pub client_id: String,
    /// Client secret of the OAuth application.
    pub client_secret: String,
}

/// The arguments of `list_git_repositories`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListGitRepositoriesParams {
    /// Connection ID to query repositories from.
    pub id: String,
    /// Optional search substring to filter repository names.
    pub search: Option<String>,
    /// Optional page number starting from 1.
    pub page: Option<u32>,
}

/// The arguments of `list_git_branches`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListGitBranchesParams {
    /// Connection ID that has access to the repository.
    pub id: String,
    /// Full repository name in "owner/repo" format.
    pub repository: String,
}

/// The arguments of `create_account`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateAccountParams {
    /// Username for the account.
    pub username: String,
    /// Password for the account (minimum 8 characters).
    pub password: String,
    /// Role of the account: "admin", "editor", or "viewer" (default: "admin").
    pub role: Option<String>,
    /// Optional list of proxy IDs the account is restricted to.
    pub proxies: Option<Vec<String>>,
    /// Whether to enable TOTP two-factor authentication.
    pub totp: Option<bool>,
}

/// The arguments of `update_account`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateAccountParams {
    /// Username of the account to update.
    pub username: String,
    /// Role of the account: "admin", "editor", or "viewer".
    pub role: String,
    /// Optional new password. Omitted or empty keeps current password.
    pub password: Option<String>,
    /// Optional list of proxy IDs the account is restricted to.
    pub proxies: Option<Vec<String>>,
}

/// The arguments of `delete_account`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteAccountParams {
    /// Username of the account to delete.
    pub username: String,
}

/// The arguments of `create_token`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateAuthTokenParams {
    /// Descriptive label for the token (e.g. "CI Deploy Token").
    pub name: String,
    /// Token permission scope: "read" or "edit" (default: "read").
    pub scope: Option<String>,
}

/// The arguments of `install_store_app`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct InstallStoreAppParams {
    /// The catalog ID of the app to install (e.g. "vaultwarden", "uptime-kuma", "plausible").
    pub app_id: String,
    /// Optional custom name for the deployed application. Defaults to the catalog id.
    pub name: Option<String>,
    /// Optional target node id where the app should be installed. Defaults to "local".
    pub target: Option<String>,
    /// Optional domains to route to this app.
    pub domains: Option<Vec<String>>,
    /// Optional listener port names or ids to expose the app on (e.g. ["http", "https"]).
    pub ports: Option<Vec<String>>,
}

/// The `{ "request": {...} }` argument of `create_database`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateDatabaseParams {
    /// Database creation request (`{"name": "mydb", "engine": "postgres", "version": "postgres:17-alpine", "host_port": 5432}`).
    pub request: r3v3rs3_api::platform::DatabaseRequest,
}

/// The arguments of `reset_database_password`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ResetDatabasePasswordParams {
    /// The id of the database.
    pub id: String,
    /// The tenant whose password changes. Omit it to change the root password.
    pub tenant: Option<String>,
}

/// The arguments of `delete_database_tenant`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteDatabaseTenantParams {
    /// The id of the database.
    pub id: String,
    /// The name of the tenant, as `list_database_tenants` returns it.
    pub tenant: String,
}

/// The arguments of `delete_database_backup`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteBackupParams {
    /// The database app id.
    pub db_id: String,
    /// The backup id.
    pub backup_id: String,
}

/// The arguments of `test_database_backup`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TestBackupParams {
    /// The database app id.
    pub db_id: String,
    /// The backup id to test.
    pub backup_id: String,
}

/// The arguments of `restore_database_pitr`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RestoreDatabasePitrParams {
    /// The database app id.
    pub id: String,
    /// Target restore timestamp in unix milliseconds (e.g. 1761642000000).
    pub target_time: u64,
    /// Optional database name if restoring as a new branched database (PITR Clone).
    pub new_name: Option<String>,
    /// Optional host port if restoring as a new branched database.
    pub host_port: Option<u16>,
}

/// The `{ "config": {...} }` argument of `update_backup_config`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateBackupConfigParams {
    /// The whole backup configuration (`{"schedule": "02:00", "keep": 7, "s3": {...}, "sftp": {...},
    /// "rsync": {...}, "restore_test_enabled": true, "heartbeat_url": "https://..."}`).
    /// `schedule` is a time of day as `HH:MM`; an empty schedule turns the backup scheduler off.
    pub config: r3v3rs3_api::platform::BackupConfig,
}

/// The `{ "request": {...} }` argument of `create_target`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddTargetParams {
    /// Target node request (`{"name": "node-1"}`).
    pub request: r3v3rs3_api::platform::TargetRequest,
}

/// The arguments of `update_target_region`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetTargetRegionParams {
    /// Target node ID.
    pub id: String,
    /// Geographical region identifier (e.g. "eu", "na", "tr", "apac") or null to clear.
    pub region: Option<String>,
}

/// The `{ "spec": {...} }` argument of `create_project`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateProjectParams {
    /// Project specification (`{"name": "my-project", "description": "...",
    /// "default_environments": ["staging", "production"]}`); the default environments are created
    /// with the project, and without them a `production` environment is created.
    pub spec: r3v3rs3_api::platform::ProjectSpec,
}

/// The `{ "group": {...} }` argument of `create_env_group`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateEnvGroupParams {
    /// Environment group specification (`{"name": "shared-env", "description": "...",
    /// "variables": [{"key": "FOO", "value": "bar", "secret": false}]}`).
    pub group: r3v3rs3_api::platform::EnvGroupSpec,
}

/// The arguments of `create_environment`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateProjectEnvironmentParams {
    /// Project ID to create the environment in.
    pub project_id: String,
    /// Name of the environment (e.g. "staging", "production", "preview").
    pub name: String,
    /// Optional description for the environment.
    pub description: Option<String>,
}

/// The arguments of `update_environment_env`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateProjectEnvParams {
    /// Environment ID to update variables for.
    pub environment_id: String,
    /// Environment variables as an array of objects (`[{"key": "FOO", "value": "bar", "secret": false}]`).
    pub variables: Vec<r3v3rs3_api::platform::EnvEntry>,
}

/// The arguments of `sync_environment_env`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SyncProjectEnvParams {
    /// Project ID owning the environments.
    pub project_id: String,
    /// Source environment ID to copy variables from.
    pub source_env_id: String,
    /// Target environment ID to receive synchronized variables.
    pub target_env_id: String,
    /// Sync mode: "sync_missing" (default), "clone_all", or "selected_keys".
    pub mode: Option<String>,
    /// Optional list of specific variable keys when mode is "selected_keys".
    pub keys: Option<Vec<String>>,
}

/// The arguments of `create_disaster_recovery_backup`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateDrBackupParams {
    /// The passphrase that encrypts the bundle with AES-256-GCM; a restore needs the same one.
    pub passphrase: String,
    /// Whether to replicate the backup to configured S3 storage.
    pub upload_s3: Option<bool>,
}

/// The arguments of `restore_disaster_recovery_backup`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RestoreDrBackupParams {
    /// Identifier of the disaster recovery backup snapshot to restore.
    pub backup_id: String,
    /// The passphrase that the bundle was created with.
    pub passphrase: String,
}

/// The arguments of `delete_disaster_recovery_backup`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteDrBackupParams {
    /// Identifier of the disaster recovery backup snapshot to delete.
    pub backup_id: String,
}

/// The arguments of `vacuum_platform_logs`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct VacuumLogsParams {
    /// Optional size threshold in megabytes. Only the containers whose log files exceed this
    /// size lose their rotated files.
    pub threshold_mb: Option<u64>,
}

/// The arguments of `update_node_alerts`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateNodeAlertsParams {
    /// Whether node resource threshold alerts are enabled.
    pub enabled: bool,
    /// Low free disk space percentage threshold (e.g. 10 for <= 10% free space).
    pub min_free_disk_percent: Option<u8>,
    /// Bandwidth spike threshold in bytes per second (e.g. 104857600 for 100 MB/s).
    pub bandwidth_spike_bytes_per_sec: Option<u64>,
    /// Maximum open file descriptor ratio percentage threshold (e.g. 80 for >= 80% limit).
    pub max_fd_ratio_percent: Option<u8>,
    /// Maximum TCP TIME_WAIT socket count threshold (e.g. 10000).
    pub max_time_wait_sockets: Option<u32>,
}

/// The arguments of `quarantine_ip`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct QuarantineIpParams {
    /// The client IP address to quarantine (IPv4 or IPv6).
    pub ip: String,
    /// Reason for quarantine.
    pub reason: Option<String>,
    /// Quarantine duration in seconds (default: 3600).
    pub duration_seconds: Option<u64>,
}

/// The arguments of `update_quarantine_config`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateQuarantineConfigParams {
    /// Whether auto-quarantine threat detection is enabled.
    pub enabled: Option<bool>,
    /// Max violations within window before ban.
    pub max_violations: Option<u32>,
    /// Sliding window duration in seconds.
    pub window_seconds: Option<u64>,
    /// Ban duration in seconds.
    pub ban_duration_seconds: Option<u64>,
    /// Status codes considered violations (e.g. [404, 403, 401]).
    pub block_status_codes: Option<Vec<u16>>,
    /// Scanner paths matched on whole path segments; each match is one violation
    /// (e.g. ["/.env", "/.git"]).
    pub scanner_patterns: Option<Vec<String>>,
    /// Addresses or CIDR networks that are never quarantined (e.g. ["198.51.100.0/24"]).
    /// Replaces the whole list; an empty list removes every entry.
    pub allowed_ips: Option<Vec<String>>,
}

/// The arguments of `update_geoip_config`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateGeoIpConfigParams {
    /// MaxMind account ID (optional).
    pub account_id: Option<String>,
    /// MaxMind license key for GeoLite2-Country downloads.
    pub license_key: Option<String>,
    /// Custom path to the GeoIP MMDB database file (optional).
    pub database: Option<String>,
    /// Whether automated background GeoIP MMDB updates are enabled.
    pub auto_update: Option<bool>,
    /// Automatic update interval in days (1 to 90).
    pub update_interval_days: Option<u32>,
}

/// The arguments of `create_error_page_theme`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddErrorPageThemeParams {
    /// Human-friendly display name of the theme.
    pub name: String,
    /// Optional description of the theme design or purpose.
    pub description: Option<String>,
    /// List of custom error pages mapped by HTTP status code (JSON array of {status, body}).
    pub pages: Option<Vec<r3v3rs3_api::proxy::CustomErrorPage>>,
}

/// The arguments of `update_error_page_theme`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateErrorPageThemeParams {
    /// Unique identifier of the error page theme.
    pub id: String,
    /// Human-friendly display name of the theme.
    pub name: Option<String>,
    /// Optional description of the theme design or purpose.
    pub description: Option<String>,
    /// List of custom error pages mapped by HTTP status code (JSON array of {status, body}).
    pub pages: Option<Vec<r3v3rs3_api::proxy::CustomErrorPage>>,
}

/// The arguments of `list_compression_dictionaries`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListCompressionDictionariesParams {
    /// The id of the proxy.
    pub proxy_id: String,
}

/// The arguments of `get_compression_dictionary` and `delete_compression_dictionary`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ProxyDictIdParams {
    /// The id of the proxy.
    pub proxy_id: String,
    /// The id of the compression dictionary.
    pub dict_id: String,
}

/// The arguments of `create_compression_dictionary`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddCompressionDictionaryParams {
    /// The id of the proxy.
    pub proxy_id: String,
    /// Human-friendly display label of the dictionary.
    pub name: String,
    /// URL path match pattern sent in `Use-As-Dictionary: match="..."` (e.g. `/api/*`).
    pub match_pattern: String,
    /// Serving path on this proxy where r3v3rs3 serves the raw bytes (e.g. `/.well-known/dict/api.dict`).
    pub path: String,
    /// Raw text, schema, or binary data content of the dictionary.
    pub raw_data: String,
}

fn resolve_mcp_grpc_target(params_target: Option<String>, proxy: &Proxy) -> Option<String> {
    params_target.or_else(|| match &proxy.kind {
        ProxyKind::Http(http) => http
            .routes
            .iter()
            .flat_map(|r| &r.servers)
            .map(|s| s.url.to_string())
            .next(),
        _ => None,
    })
}

/// The arguments of `check_proxy_grpc_health`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CheckGrpcHealthParams {
    /// The id of the proxy (optional if target_url is provided).
    pub proxy_id: Option<String>,
    /// Direct upstream target URL (e.g. `http://127.0.0.1:50051`, optional if proxy_id is provided).
    pub target_url: Option<String>,
    /// Optional gRPC service name to check. Empty string checks overall server health.
    pub service: Option<String>,
}

/// The arguments of `apply_proxy_security_header_profile`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ApplySecurityHeaderProfileParams {
    /// The id of the proxy.
    pub proxy_id: String,
    /// The security profile to apply: `strict`, `balanced`, `api_only`, or `spa_frontend`.
    pub profile: String,
}

fn parse_mcp_security_profile(
    s: &str,
) -> Result<r3v3rs3_api::security_headers::SecurityProfile, ErrorData> {
    use r3v3rs3_api::security_headers::SecurityProfile;
    match s.to_ascii_lowercase().as_str() {
        "strict" => Ok(SecurityProfile::Strict),
        "balanced" => Ok(SecurityProfile::Balanced),
        "api_only" | "api-only" | "api" => Ok(SecurityProfile::ApiOnly),
        "spa_frontend" | "spa-frontend" | "spa" => Ok(SecurityProfile::SpaFrontend),
        other => Err(ErrorData::invalid_params(
            format!(
                "unknown security profile '{other}'. Expected strict, balanced, api_only, or spa_frontend"
            ),
            None,
        )),
    }
}

/// The arguments of `update_prune_policy`. An omitted field keeps its stored value.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdatePrunePolicyParams {
    /// Whether the automatic prune runs.
    pub enabled: Option<bool>,
    /// The time between automatic prune runs, in hours or days, for example "24h", "7d" or "7days".
    pub interval: Option<String>,
    /// The disk usage percentage (10 to 99) at which an emergency prune runs at once.
    pub disk_pressure_threshold: Option<u8>,
    /// Whether to delete unused images.
    pub prune_images: Option<bool>,
    /// Whether to delete unused volumes. This deletes their data for good.
    pub prune_volumes: Option<bool>,
    /// Whether to delete the build cache.
    pub prune_build_cache: Option<bool>,
    /// Whether to delete stopped containers.
    pub prune_containers: Option<bool>,
    /// Whether to delete the rotated container log files during a prune.
    pub prune_logs: Option<bool>,
    /// Delete only the rotated files of the container logs larger than this many MB.
    pub log_vacuum_threshold_mb: Option<u64>,
    /// The MB of build cache to keep; older cache goes first.
    pub build_cache_keep_storage_mb: Option<u64>,
    /// The number of newest build images kept per app for rollbacks (default 5).
    pub kept_builds_count: Option<u32>,
    /// Delete only dangling images and layers (true) or every unused image (false).
    pub dangling_layers_only: Option<bool>,
}

fn apply_prune_policy_overrides(
    policy: &mut r3v3rs3_api::platform::PrunePolicy,
    params: &UpdatePrunePolicyParams,
) {
    if let Some(en) = params.enabled {
        policy.enabled = en;
    }
    if let Some(ref iv) = params.interval {
        policy.interval = iv.clone();
    }
    if let Some(dp) = params.disk_pressure_threshold {
        policy.disk_pressure_threshold = Some(dp);
    }
    apply_prune_flags(policy, params);
}

fn apply_prune_flags(
    policy: &mut r3v3rs3_api::platform::PrunePolicy,
    params: &UpdatePrunePolicyParams,
) {
    if let Some(img) = params.prune_images {
        policy.prune_images = img;
    }
    if let Some(vol) = params.prune_volumes {
        policy.prune_volumes = vol;
    }
    if let Some(bc) = params.prune_build_cache {
        policy.prune_build_cache = bc;
    }
    if let Some(cnt) = params.prune_containers {
        policy.prune_containers = cnt;
    }
    if let Some(logs) = params.prune_logs {
        policy.prune_logs = logs;
    }
    apply_prune_retention(policy, params);
}

fn apply_prune_retention(
    policy: &mut r3v3rs3_api::platform::PrunePolicy,
    params: &UpdatePrunePolicyParams,
) {
    if let Some(th) = params.log_vacuum_threshold_mb {
        policy.log_vacuum_threshold_mb = Some(th);
    }
    if let Some(mb) = params.build_cache_keep_storage_mb {
        policy.build_cache_keep_storage_mb = Some(mb);
    }
    if let Some(kb) = params.kept_builds_count {
        policy.kept_builds_count = Some(kb);
    }
    if let Some(dangling) = params.dangling_layers_only {
        policy.dangling_layers_only = dangling;
    }
}

/// The caller of the request, which the Bearer middleware inserted.
fn caller_of(parts: &Parts) -> Result<Caller, ErrorData> {
    parts
        .extensions
        .get::<Caller>()
        .cloned()
        .ok_or_else(|| ErrorData::internal_error("the caller is missing", None))
}

/// The scope of the access token.
fn scope_of(parts: &Parts) -> Result<&str, ErrorData> {
    parts
        .extensions
        .get::<McpScope>()
        .map(|scope| scope.0.as_str())
        .ok_or_else(|| ErrorData::internal_error("the scope is missing", None))
}

/// Rejects a call whose token has no `edit` scope.
fn require_edit(parts: &Parts) -> Result<(), ErrorData> {
    if scope_of(parts)?
        .split_whitespace()
        .any(|scope| scope == "edit")
    {
        Ok(())
    } else {
        Err(ErrorData::invalid_params(
            "the access token has no edit scope, approve the client with the edit scope again",
            None,
        ))
    }
}

/// The error of an RPC method as a tool error. The text of the API errors is public.
fn tool_error(err: Error) -> ErrorData {
    ErrorData::internal_error(err.to_string(), None)
}

/// The largest response body a tool reads from a handler that answers with raw bytes.
const MAX_TOOL_BODY: usize = 16 * 1024 * 1024;

/// The largest app file that the file tools read or write as text.
const MAX_TOOL_FILE: usize = 1024 * 1024;

/// The body of a raw handler response as text.
async fn response_text(response: axum::response::Response) -> Result<String, ErrorData> {
    small_text(response, MAX_TOOL_BODY).await
}

/// The body of a raw handler response as UTF-8 text of at most `limit` bytes.
async fn small_text(response: axum::response::Response, limit: usize) -> Result<String, ErrorData> {
    let bytes = axum::body::to_bytes(response.into_body(), limit)
        .await
        .map_err(|_| {
            ErrorData::invalid_params(
                format!("the content is larger than {limit} bytes; use the CLI to download it"),
                None,
            )
        })?;
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        ErrorData::invalid_params(
            "the content is not UTF-8 text; use the CLI to download it",
            None,
        )
    })
}

/// An app and one of its volumes.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct VolumeParams {
    /// The id of the app.
    pub id: String,
    /// The name of the volume.
    pub volume: String,
}

/// An app, one of its volumes and a stored backup of it.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct VolumeBackupParams {
    /// The id of the app.
    pub id: String,
    /// The name of the volume.
    pub volume: String,
    /// The id of the backup.
    pub backup_id: String,
}

/// An app and a path inside one of its volumes.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AppFileParams {
    /// The id of the app.
    pub id: String,
    /// The absolute path inside the container, for example `/data/app.db`.
    pub path: String,
}

impl AppFileParams {
    fn query(self) -> Result<(ShortId, r3v3rs3_api::platform::FilePathQuery), ErrorData> {
        let id = parse_id(&self.id)?;
        Ok((id, r3v3rs3_api::platform::FilePathQuery { path: self.path }))
    }
}

/// The arguments of `upload_app_file`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UploadAppFileParams {
    /// The id of the app.
    pub id: String,
    /// The absolute path inside the container.
    pub path: String,
    /// The new content of the file, as UTF-8 text.
    pub content: String,
}

/// Parses an id string into a typed ShortId.
fn parse_id(id: &str) -> Result<ShortId, ErrorData> {
    id.parse()
        .map_err(|err: Error| ErrorData::invalid_params(err.to_string(), None))
}

/// Parses a snake_case choice of a tool argument, such as a role or a mode. An unknown value is
/// an error, never a fallback to another choice.
fn parse_choice<T: serde::de::DeserializeOwned>(field: &str, value: &str) -> Result<T, ErrorData> {
    serde_json::from_value(serde_json::Value::String(value.to_string()))
        .map_err(|_| ErrorData::invalid_params(format!("unknown {field} `{value}`"), None))
}

/// Parses an optional choice; an omitted argument takes `default`.
fn parse_optional_choice<T: serde::de::DeserializeOwned>(
    field: &str,
    value: Option<&str>,
    default: T,
) -> Result<T, ErrorData> {
    value.map_or(Ok(default), |value| parse_choice(field, value))
}

/// The validated sync request of `sync_environment_env`.
fn sync_request(
    params: &SyncProjectEnvParams,
) -> Result<r3v3rs3_api::platform::EnvironmentSyncRequest, ErrorData> {
    let request = r3v3rs3_api::platform::EnvironmentSyncRequest {
        source_env_id: parse_id(&params.source_env_id)?,
        target_env_id: parse_id(&params.target_env_id)?,
        mode: parse_optional_choice(
            "mode",
            params.mode.as_deref(),
            r3v3rs3_api::platform::EnvironmentSyncMode::SyncMissing,
        )?,
        keys: parse_values(params.keys.iter().flatten())?,
    };
    request.validate().map_err(any_error)?;
    Ok(request)
}

/// Parses every value of a tool argument; an invalid value is an error, never dropped.
fn parse_values<T, C>(values: impl IntoIterator<Item = impl AsRef<str>>) -> Result<C, ErrorData>
where
    T: std::str::FromStr<Err = Error>,
    C: FromIterator<T>,
{
    values
        .into_iter()
        .map(|value| {
            value
                .as_ref()
                .parse()
                .map_err(|err: Error| ErrorData::invalid_params(err.to_string(), None))
        })
        .collect()
}

/// Parses an optional value; an invalid value is an error, never `None`.
fn parse_optional_value<T: std::str::FromStr<Err = Error>>(
    value: Option<&str>,
) -> Result<Option<T>, ErrorData> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|err: Error| ErrorData::invalid_params(err.to_string(), None))
        })
        .transpose()
}

/// Formats any displayable error into an ErrorData.
fn any_error(err: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(err.to_string(), None)
}

/// The text result of a tool, as pretty JSON.
fn json_result(value: impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(&value)
        .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// A theme without the HTML of its pages: each page gives its status code and body size.
fn theme_summary(theme: &r3v3rs3_api::error_page_theme::ErrorPageTheme) -> serde_json::Value {
    let pages: Vec<_> = theme
        .pages
        .iter()
        .map(|page| json!({ "status": page.status, "body_bytes": page.body.len() }))
        .collect();
    json!({
        "id": theme.id,
        "name": theme.name,
        "description": theme.description,
        "pages": pages,
    })
}

/// Constructs an AppRequest from a catalog store app and user parameters.
fn build_store_app_request(
    store_app: &r3v3rs3_api::store::StoreApp,
    params: &InstallStoreAppParams,
) -> Result<r3v3rs3_api::platform::AppRequest, ErrorData> {
    use r3v3rs3_api::container::{AppName, ServiceName};
    use r3v3rs3_api::git::GitRef;
    use r3v3rs3_api::platform::{AppSource, AppSpec, LOCAL_TARGET};
    use r3v3rs3_api::subject_name::SubjectName;

    let name_str = params.name.as_deref().unwrap_or(&store_app.id);
    let name: AppName = name_str
        .parse()
        .map_err(|e: Error| ErrorData::invalid_params(e.to_string(), None))?;
    let target_str = params.target.as_deref().unwrap_or(LOCAL_TARGET);
    let target: ShortId = target_str
        .parse()
        .map_err(|e: Error| ErrorData::invalid_params(e.to_string(), None))?;

    let domains: Vec<SubjectName> = parse_values(params.domains.iter().flatten())?;

    let source = if store_app.is_compose() {
        let service: ServiceName = store_app
            .service
            .as_deref()
            .unwrap_or("web")
            .parse()
            .map_err(|e: Error| ErrorData::invalid_params(e.to_string(), None))?;
        AppSource::Compose {
            repository: parse_optional_value(store_app.repository.as_deref())?,
            branch: parse_optional_value(store_app.branch.as_deref())?.unwrap_or_else(GitRef::main),
            file: parse_optional_value(store_app.file.as_deref())?,
            service,
            connection: None,
            compose_yaml: None,
        }
    } else {
        let image = store_app
            .image
            .as_deref()
            .unwrap_or_default()
            .parse()
            .map_err(|e: Error| ErrorData::invalid_params(e.to_string(), None))?;
        AppSource::Image { image }
    };

    let spec = AppSpec {
        source,
        port: store_app.port,
        domains,
        ports: params.ports.clone().unwrap_or_default(),
        health_check_path: store_app.health_check_path.clone(),
        volumes: Vec::new(),
        restart: Default::default(),
        limits: Default::default(),
        host_port: None,
        database: None,
        uses_database: None,
        replica_of: None,
        auto_deploy: false,
        require_approval: false,
        deploy_window: None,
        tag_trigger: None,
        watch_paths: Vec::new(),
        host_ports: Vec::new(),
        store_app_id: Some(store_app.id.clone()),
        targets: Vec::new(),
        load_balancing: None,
        alert_rules: None,
        sticky: None,
        git_config: None,
        env_groups: Vec::new(),
        preview: None,
        preview_of: None,
        buildkit: None,
        bot_blocker: false,
        grpc_web: false,
        frame_protection: true,
        project_id: None,
        environment_id: None,
        lifecycle_hooks: None,
    };

    Ok(r3v3rs3_api::platform::AppRequest { name, target, spec })
}

#[tool_router]
impl McpServer {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    // ==========================================
    // Group 1: Server & Monitoring
    // ==========================================

    /// The version, the target and the state of the server.
    #[tool(
        description = "Show the release version and the build target of this r3v3rs3 server, and the state of the worker processes that serve the ports (in `workers = off` mode the master serves them itself). Use it first to check which server and version you talk to. Takes no arguments. Read-only; any account can call it."
    )]
    async fn server_status(
        &self,
        Extension(parts): Extension<Parts>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let _ = ctx;
        let caller = caller_of(&parts)?;
        let app_info = json!({
            "version": self.state.data.lock().await.app_info.version,
            "target": self.state.data.lock().await.app_info.target,
        });
        let workers: r3v3rs3_api::app::WorkerStatusReport = *self
            .state
            .call(&caller, GetWorkers)
            .await
            .map_err(tool_error)?;
        json_result(json!({
            "app": app_info,
            "workers": workers,
        }))
    }

    /// Get the current status and step of the server self-upgrade.
    #[tool(
        description = "Show the stage of the server self-upgrade that runs at the moment: `checking_release`, `downloading`, `installing`, `handing_over` or `done`; `null` when no upgrade runs. Poll it after `upgrade_server` until the stage is `done`. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_upgrade_status(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let status = super::upgrade::status(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(status.0)
    }

    /// Run server self-upgrade to the latest release.
    #[tool(
        description = "Upgrade this server to the newest GitHub release: download and install the binary, start the new version and hand the listening sockets over to it, so open connections are not dropped. The call returns the release tag as soon as the upgrade starts; follow it with `get_upgrade_status`. Takes no arguments. Needs a token with the edit scope and an admin account."
    )]
    async fn upgrade_server(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        caller.authorize(Permission::Admin).map_err(tool_error)?;
        let started = super::upgrade::upgrade(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(started.0)
    }

    /// The settings that the admin API can change.
    #[tool(
        description = "Show the global settings of the server (public URL, ACME, notifications, discovery, MCP, platform and the other settings of the Settings page). Secret values such as keys and passwords are masked. Takes no arguments. Read-only; admin accounts only."
    )]
    async fn get_config(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let config: r3v3rs3_api::app::AppConfig = *self
            .state
            .call(&caller, GetConfig)
            .await
            .map_err(tool_error)?;
        json_result(config.masked())
    }

    /// List recent audit log entries.
    #[tool(
        description = "List the newest entries of the audit log: who changed what, when, from which client address, and the action name (for example `add_proxy` or `deploy_app`). `limit` caps the number of entries (default 50). Read-only; admin accounts only."
    )]
    async fn list_audit_log(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AuditLogParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let audit = self
            .state
            .call(&caller, GetAuditLog)
            .await
            .map_err(tool_error)?;
        let query = r3v3rs3_api::audit::AuditQuery {
            limit: params.limit,
            ..Default::default()
        };
        let filter = crate::audit::AuditFilter::new(&query, crate::clock::unix_ms());
        let entries = audit.query(&filter).await.map_err(|err| {
            ErrorData::internal_error(format!("failed to read audit log: {err}"), None)
        })?;
        json_result(entries)
    }

    /// Process and system performance metrics.
    #[tool(
        description = "Show the current resource use of the server process and host: open file descriptors against their limit, memory use and the event loop lag of the runtime. Use it to check whether the server is overloaded. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_system_metrics(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let _caller = caller_of(&parts)?;
        let metrics = crate::system_metrics::collect_system_metrics();
        json_result(metrics)
    }

    /// Real-time system anomaly detection.
    #[tool(
        description = "Show the active and recent anomalies of the background monitor (high p99 latency, high 5xx error rate, request rate spike, event loop lag), the current requests per second and event loop lag, and the alert thresholds it compares against. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_system_anomalies(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let snapshot = super::anomaly::get_snapshot(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(snapshot.0)
    }

    /// Banned client IP addresses.
    #[tool(
        description = "List the client addresses that the sign-in brute-force protection bans at the moment, with the failed sign-in count, the end time and the remaining seconds of each ban. These bans block sign-in to the panel only; `list_quarantined_ips` lists the addresses blocked from all traffic. Takes no arguments. Read-only; admin accounts only."
    )]
    async fn list_banned_ips(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let banned = super::ban::list_banned_ips(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(banned.0)
    }

    /// Unban a client IP address.
    #[tool(
        description = "Lift the sign-in ban of one client address (`ip`, IPv4 or IPv6), so it can sign in to the panel again at once. It does not release a quarantine; use `unquarantine_ip` for that. Needs a token with the edit scope and an admin account."
    )]
    async fn unban_ip(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let _ = super::ban::unban_ip(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(params.ip.clone()),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "unbanned": params.ip }))
    }

    /// Quarantined client IP addresses.
    #[tool(
        description = "List the client addresses in quarantine at the moment, with the reason and the end time. A quarantined address gets no answer on any port. Addresses enter quarantine automatically (scanner paths, repeated error status codes) or through `quarantine_ip`. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_quarantined_ips(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let quarantined = super::quarantine::list(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(quarantined.0)
    }

    /// Place a client IP into quarantine.
    #[tool(
        description = "Put one client address (`ip`) into quarantine at once, so it gets no answer on any port until the quarantine ends. `reason` is shown in the quarantine list, `duration_seconds` sets the length (default 3600). Never quarantine your own address: it also blocks the panel and this MCP endpoint. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn quarantine_ip(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<QuarantineIpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let ip = params
            .ip
            .parse::<std::net::IpAddr>()
            .map_err(|e| ErrorData::invalid_params(format!("invalid IP address: {e}"), None))?;
        let request = r3v3rs3_api::quarantine::QuarantineIpRequest {
            ip,
            reason: params.reason,
            duration_seconds: params.duration_seconds,
        };
        let res = super::quarantine::add(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(res.0)
    }

    /// Release an IP address from quarantine.
    #[tool(
        description = "Release one client address (`ip`) from quarantine before its end time, so its traffic is served again. To keep it from returning to quarantine, add it to `allowed_ips` with `update_quarantine_config`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn unquarantine_ip(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let _ = super::quarantine::delete(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(params.ip.clone()),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "unquarantined": params.ip }))
    }

    /// Get auto-quarantine threat detection configuration.
    #[tool(
        description = "Show the rules of the automatic quarantine: whether it is on, how many violations within which window trigger it, the quarantine length, the status codes and scanner paths that count as violations, and the addresses that are never quarantined. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_quarantine_config(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let config = super::quarantine::get_config(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(config.0)
    }

    /// Update auto-quarantine threat detection configuration.
    #[tool(
        description = "Change the rules of the automatic quarantine and save them; they apply at once. Pass only the fields to change, every omitted field keeps its value. `allowed_ips` replaces the whole list of never-quarantined addresses and CIDR networks (an empty list removes all of them), so read the current list with `get_quarantine_config` first. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_quarantine_config(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateQuarantineConfigParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let current = super::quarantine::get_config(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller.clone()),
        )
        .await
        .map_err(any_error)?;
        let mut config = *current.0;
        apply_quarantine_overrides(&mut config, params)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let _ = super::quarantine::update_config(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(config),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "status": "updated" }))
    }

    /// The host firewall table that r3v3rs3 keeps.
    #[tool(
        description = "Show the host firewall table (nftables `inet r3v3rs3`) that r3v3rs3 keeps: whether it is enabled, the TCP and UDP ports it opens (`tcp/443`, `udp/49160-49240`), warnings about other firewalls of the host that can still drop these ports, and the error of the last update. Turn it on and set extra ports with `update_firewall`. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_firewall_status(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let status = super::firewall::status(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(status.0)
    }

    /// Export the configuration backup.
    #[tool(
        description = "Export the configuration backup: the settings, ports, proxies, certificates requests and other stored configuration, with secrets masked and accounts left out. Pass the whole answer to `restore_config` to restore it. Takes no arguments. Needs an admin account."
    )]
    async fn backup_config(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let backup = super::config::backup(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(backup.0)
    }

    /// Restore a configuration backup.
    #[tool(
        description = "Restore a configuration backup (`backup`, the answer of `backup_config`). The stored configuration is replaced by the backup; masked secrets keep their current values. Needs a token with the edit scope and an admin account."
    )]
    async fn restore_config(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<RestoreConfigParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let backup: r3v3rs3_api::config_backup::ConfigBackup = json_arg(params.backup)?;
        let _ = super::config::restore(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(backup),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "restored": true }))
    }

    /// Send a test notification to the saved webhook.
    #[tool(
        description = "Send a test notification to the notification webhook of the saved settings. It fails with `notification_webhook_missing` when no webhook is saved, and with the delivery error when the webhook does not accept it. Takes no arguments. Needs a token with the edit scope and an admin account."
    )]
    async fn test_notification(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let _ = super::config::test_notification(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "delivered": true }))
    }

    /// Turn the host firewall table on or off and set its extra ports.
    #[tool(
        description = "Turn the host firewall table (nftables `inet r3v3rs3`) on or off (`enabled`) and replace its extra ports (`extra_ports`, such as [\"udp/51820\", \"tcp/8443\"]; an empty list clears them). Omitted fields keep their value. The table always opens the SSH ports and the ports of r3v3rs3; check the result and the warnings with `get_firewall_status`. Needs a token with the edit scope and an admin account."
    )]
    async fn update_firewall(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<SetFirewallParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let current = super::config::get(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller.clone()),
        )
        .await
        .map_err(any_error)?;
        let mut config = *current.0;
        apply_firewall_params(&mut config.firewall, params)?;
        let _ = super::config::put(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(config),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "status": "updated" }))
    }

    /// Get the current status and metadata of the loaded GeoIP database.
    #[tool(
        description = "Show the GeoIP country database that country rules and GeoIP steering use: whether one is loaded, its path, size and build date, the MaxMind account and whether a license key is set (the key itself is not shown), and the automatic update settings. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_geoip_status(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let status = super::geoip::status(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(status.0)
    }

    /// Update GeoIP configuration and MaxMind license key.
    #[tool(
        description = "Change the GeoIP settings: MaxMind `account_id`, `license_key`, the `database` file path, `auto_update` and `update_interval_days` (1 to 90). Omitted fields keep their value; an empty `license_key` removes the stored key. It does not download a database; call `update_geoip_database` for that. Returns the new GeoIP status. Needs a token with the edit scope and an admin account."
    )]
    async fn update_geoip_config(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateGeoIpConfigParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let current = super::geoip::status(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller.clone()),
        )
        .await
        .map_err(any_error)?;
        let req = build_geoip_update_request(&current.0, params);
        let updated = super::geoip::update_config(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(req),
        )
        .await
        .map_err(any_error)?;
        json_result(updated.0)
    }

    /// Download and hot-reload the latest GeoLite2-Country database from MaxMind.
    #[tool(
        description = "Download the newest GeoLite2-Country database from MaxMind now, verify it and load it without a restart. Needs a MaxMind license key (set it with `update_geoip_config`). Returns the new GeoIP status. Takes no arguments. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_geoip_database(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let status = super::geoip::update_database(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(status.0)
    }

    // ==========================================
    // Group 2: Ports
    // ==========================================

    /// The ports with their state.
    #[tool(
        description = "List every port (listener) of the server with its id, name, listen address in multiaddr form (for example `/ip4/0.0.0.0/tcp/443/https`), TLS termination settings and current state. A port is the listening socket; a proxy names the ids of the ports it serves in its `ports` list. Use the ids from here for the other port tools and for `create_proxy`. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_ports(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let ports: Vec<r3v3rs3_api::port::PortEntry> = *self
            .state
            .call(&caller, GetPortList)
            .await
            .map_err(tool_error)?;
        json_result(ports)
    }

    /// One port by id.
    #[tool(
        description = "Show the full configuration of one port by its `id`: name, active flag, listen address and all options, including TLS termination (server names, client certificate authentication and its CA certificates, 0-RTT early data). Use the output as the starting point of `update_port`. Read-only; any account can call it."
    )]
    async fn get_port(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let port: r3v3rs3_api::port::PortEntry = *self
            .state
            .call(&caller, GetPort { id })
            .await
            .map_err(tool_error)?;
        json_result(port)
    }

    /// Port runtime socket and TLS status.
    #[tool(
        description = "Show the runtime state of one port by its `id`: `listening`, `inactive`, `address_already_in_use`, `permission_denied`, `address_not_available` or `error`, and since when it listens. Use it after `create_port`, `update_port` or `reset_port` to confirm that the listener is up. Read-only; any account can call it."
    )]
    async fn get_port_status(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let status: r3v3rs3_api::port::PortStatus = *self
            .state
            .call(&caller, GetPortStatus { id })
            .await
            .map_err(tool_error)?;
        json_result(status)
    }

    /// Create a listening port.
    #[tool(
        description = "Create a port and start listening on it. `port` is the same JSON object as the body of POST /api/ports, for example {\"name\": \"https\", \"listen\": \"/ip4/0.0.0.0/tcp/443/https\"}; the protocol at the end of `listen` (tcp, tls, http, https, udp, quic) selects the handler. A plain TCP or UDP port can name a range such as `/ip4/0.0.0.0/udp/49160-49240` (at most 16384 ports; its proxies need upstream ranges of the same size, and each port goes to the upstream port at its position), and `opts.transparent: true` makes the upstream servers see the client address. Returns the name of the new port; read its id with `list_ports`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_port(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddPortParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let entry: r3v3rs3_api::port::Port = params.port;
        let name = entry.name.clone();
        self.state
            .call(&caller, AddPort { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "created": name }))
    }

    /// Update an existing port configuration.
    #[tool(
        description = "Replace the whole configuration of an existing port. `entry` is the port object with its `id`, as `get_port` returns it, with your changes applied; fields you leave out fall back to their defaults, so start from the output of `get_port`. The new settings apply at once. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_port(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdatePortParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let entry: r3v3rs3_api::port::PortEntry = params.entry;
        self.state
            .call(&caller, UpdatePort { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "updated": true }))
    }

    /// Delete a port by its id.
    #[tool(
        description = "Delete a port by its `id` and close its listener. The proxies are reloaded, so a proxy that listed the port is no longer served on it. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_port(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        self.state
            .call(&caller, DeletePort { id })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }

    /// Reset and reload listener socket of a port.
    #[tool(
        description = "End every open connection of a port by its `id` (TCP and HTTP connections close, UDP sessions are dropped). The listener keeps running and the configuration does not change; clients reconnect on their own. Use it to drop stuck or unwanted connections at once. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn reset_port(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        self.state
            .call(&caller, ResetPort { id })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "reset": id.to_string() }))
    }

    /// Turn the transparent option of a TCP or UDP port on or off.
    #[tool(
        description = "Turn the transparent option on or off for one TCP or UDP port (`id`, `enabled`). A transparent port opens its upstream connections from the client address, so the upstream server (a TURN server, for example) sees the real client IP; r3v3rs3 keeps the nftables table and policy route for the replies. It needs CAP_NET_ADMIN on the server, fails on an HTTP or QUIC port, and fails when a proxy of the port has a loopback upstream server. Other port settings stay unchanged. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_port_transparent(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<SetPortTransparentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let mut entry: r3v3rs3_api::port::PortEntry = *self
            .state
            .call(&caller, GetPort { id })
            .await
            .map_err(tool_error)?;
        entry.port.opts.transparent = params.enabled;
        self.state
            .call(&caller, UpdatePort { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "id": id.to_string(), "transparent": params.enabled }))
    }

    /// Enable or disable TLS 1.3 0-RTT early data on a port.
    #[tool(
        description = "Turn TLS 1.3 0-RTT early data on or off for one port (`id`, `enabled`). Early data lets a returning client send its first request in the handshake; r3v3rs3 guards it against replay. Enabling fails on a port without TLS termination. Other port settings stay unchanged. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_port_early_data(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<SetPortEarlyDataParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let mut entry: r3v3rs3_api::port::PortEntry = *self
            .state
            .call(&caller, GetPort { id })
            .await
            .map_err(tool_error)?;
        if let Some(tls) = entry.port.opts.tls_termination.as_mut() {
            tls.early_data = params.enabled;
        } else if params.enabled {
            return Err(ErrorData::invalid_params(
                "Port does not have TLS termination configured",
                None,
            ));
        }
        self.state
            .call(&caller, UpdatePort { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "id": id.to_string(), "early_data": params.enabled }))
    }

    /// List host network interfaces.
    #[tool(
        description = "List the network interfaces of the host with their IP addresses and MAC address, to pick the address a port listens on. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_network_interfaces(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let ifaces: Vec<r3v3rs3_api::port::NetworkInterface> = *self
            .state
            .call(&caller, GetNetworkInterfaceList)
            .await
            .map_err(tool_error)?;
        json_result(ifaces)
    }

    // ==========================================
    // Group 3: Proxies
    // ==========================================

    /// The proxies with their state.
    #[tool(
        description = "List every proxy with its id, name, active flag, the ids of the ports it serves and its full configuration (`protocol` is `tcp`, `udp` or `http`; an HTTP proxy holds its `vhosts`, `routes` with upstream `servers`, headers, cache, auth and limits). Proxies created from Docker labels, etcd or the deployment platform are listed too. An account with a proxy list sees only the proxies on its list. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_proxies(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let proxies: Vec<ProxyEntry> = *self
            .state
            .call(&caller, GetProxyList)
            .await
            .map_err(tool_error)?;
        json_result(proxies)
    }

    /// One proxy by id.
    #[tool(
        description = "Show the full configuration of one proxy by its `id`. Use the output as the starting point of `update_proxy`, because that tool replaces the whole configuration. An account with a proxy list gets `id_not_found` for a proxy outside its list. Read-only; any account can call it."
    )]
    async fn get_proxy(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let proxy: ProxyEntry = *self
            .state
            .call(&caller, GetProxy { id })
            .await
            .map_err(tool_error)?;
        json_result(proxy)
    }

    /// Proxy runtime health and upstream connection status.
    #[tool(
        description = "Show the runtime state of one proxy by its `id`: whether it is configured and served, the health of each upstream server, and the resolved targets of DNS SRV upstreams. Use it to find out why a site does not answer. Read-only; any account can call it."
    )]
    async fn get_proxy_status(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let status = self
            .state
            .call(&caller, GetProxyStatus { id })
            .await
            .map_err(tool_error)?;
        json_result(status)
    }

    /// Check upstream gRPC health using grpc.health.v1.Health/Check protocol.
    #[tool(
        description = "Call `grpc.health.v1.Health/Check` on a gRPC server now and report whether it serves, the gRPC status and the latency. Give `target_url` (for example `http://127.0.0.1:50051`; `https://` uses TLS, anything else plaintext HTTP/2) or `proxy_id` (the first upstream server of its routes is checked; `target_url` wins when both are given). `service` checks one service; empty checks the whole server. The timeout is 5 seconds. Read-only; any account can call it."
    )]
    async fn check_proxy_grpc_health(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CheckGrpcHealthParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let target = if let Some(ref proxy_id) = params.proxy_id {
            let id: ShortId = parse_id(proxy_id)?;
            let entry: ProxyEntry = *self
                .state
                .call(&caller, GetProxy { id })
                .await
                .map_err(tool_error)?;
            resolve_mcp_grpc_target(params.target_url, &entry.proxy)
        } else {
            params.target_url
        };

        let target_url = target.ok_or_else(|| {
            ErrorData::invalid_params(
                "either proxy_id with configured upstream servers or target_url is required",
                None,
            )
        })?;

        let service = params.service.unwrap_or_default();
        let is_plaintext = !target_url.starts_with("https://");
        let client = crate::grpc::health::build_grpc_health_client(
            is_plaintext,
            std::time::Duration::from_secs(5),
        )
        .map_err(|err| ErrorData::internal_error(err, None))?;

        let res = crate::grpc::health::check_grpc_health(&client, &target_url, &service)
            .await
            .map_err(|err| ErrorData::internal_error(err, None))?;

        json_result(res)
    }

    /// List all predefined OWASP security hardening profiles.
    #[tool(
        description = "List the built-in security header profiles (`strict`, `balanced`, `api_only`, `spa_frontend`) with the response headers each one sets (for example Content-Security-Policy, Strict-Transport-Security, X-Frame-Options). Use it to choose a profile for `apply_proxy_security_header_profile`. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_security_header_profiles(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let _ = caller_of(&parts)?;
        let profiles: Vec<r3v3rs3_api::security_headers::SecurityProfileInfo> =
            r3v3rs3_api::security_headers::SecurityProfile::all()
                .iter()
                .map(r3v3rs3_api::security_headers::SecurityProfile::to_info)
                .collect();
        json_result(profiles)
    }

    /// Analyze the security headers of an HTTP proxy and return a security audit score.
    #[tool(
        description = "Grade the response header rules configured on one HTTP proxy (`id`) from A+ to F, with the score and, for each security header, whether it is set and which risk a missing one leaves open. It reads the configuration, not live responses. Fails for TCP and UDP proxies. Read-only; any account can call it."
    )]
    async fn get_proxy_security_headers(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let entry: ProxyEntry = *self
            .state
            .call(&caller, GetProxy { id })
            .await
            .map_err(tool_error)?;

        match entry.proxy.kind {
            r3v3rs3_api::proxy::ProxyKind::Http(http) => {
                let audit =
                    r3v3rs3_api::security_headers::analyze_response_rules(&http.headers.response);
                json_result(audit)
            }
            _ => Err(ErrorData::invalid_params(
                format!("proxy {} is not an HTTP proxy", id),
                None,
            )),
        }
    }

    /// Apply a predefined OWASP security profile to an HTTP proxy.
    #[tool(
        description = "Set the security response headers of one HTTP proxy (`proxy_id`) to a built-in profile: `strict`, `balanced`, `api_only` or `spa_frontend` (see `list_security_header_profiles`). It changes the live proxy at once and returns the new grade, as `get_proxy_security_headers` gives it. A strict Content-Security-Policy can break a site that loads scripts from other origins. Needs a token with the edit scope and an editor or admin account; an editor with a proxy list changes only the proxies on its list."
    )]
    async fn apply_proxy_security_header_profile(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ApplySecurityHeaderProfileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let profile = parse_mcp_security_profile(&params.profile)?;
        let id: ShortId = parse_id(&params.proxy_id)?;
        let audit = self
            .state
            .call(&caller, ApplyProxySecurityHeadersProfile { id, profile })
            .await
            .map_err(tool_error)?;
        json_result(*audit)
    }

    /// Returns the HTTP/2 and HTTP/3 stream priority configuration of a proxy.
    #[tool(
        description = "Show the HTTP/2 and HTTP/3 stream priority settings (RFC 9218) of one HTTP proxy (`id`): whether adaptive priority is on, whether a `Priority` header is sent upstream, and whether adaptive concurrency backpressure is on. Fails for TCP and UDP proxies. Read-only; any account can call it."
    )]
    async fn get_proxy_stream_priority(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let entry: ProxyEntry = *self
            .state
            .call(&caller, GetProxy { id })
            .await
            .map_err(tool_error)?;
        match entry.proxy.kind {
            r3v3rs3_api::proxy::ProxyKind::Http(http) => json_result(http.stream_priority),
            _ => Err(ErrorData::invalid_params(
                format!("proxy {} is not an HTTP proxy", id),
                None,
            )),
        }
    }

    /// Update the stream priority configuration of an HTTP proxy.
    #[tool(
        description = "Turn RFC 9218 adaptive stream priority on or off for one HTTP proxy (`id`, `enabled`). `inject_header` sends the `Priority` header to the upstream servers; `adaptive_concurrency` limits concurrent streams when the upstream slows down. An omitted optional flag keeps its value. Applies at once. Needs a token with the edit scope and an editor or admin account; an editor with a proxy list changes only the proxies on its list."
    )]
    async fn update_proxy_stream_priority(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<SetStreamPriorityParams>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        self.state
            .call(
                &caller,
                UpdateProxyStreamPriority {
                    id,
                    mutation: StreamPriorityMutation::Patch {
                        enabled: params.enabled,
                        inject_header: params.inject_header,
                        adaptive_concurrency: params.adaptive_concurrency,
                    },
                },
            )
            .await
            .map_err(tool_error)?;
        json_result(json!({ "id": id.to_string(), "stream_priority_enabled": params.enabled }))
    }

    /// Get HTTP stream priority and adaptive concurrency statistics.
    #[tool(
        description = "Show the server-wide counters of the stream priority scheduler since the start: streams per RFC 9218 urgency class and how often adaptive backpressure throttled streams. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_traffic_priority_stats(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let _ = caller_of(&parts)?;
        json_result(r3v3rs3_api::priority::GLOBAL_STREAM_CONTROLLER.stats())
    }

    /// A new proxy from the JSON form of the admin API.
    #[tool(
        description = "Create a proxy and start serving it on its ports at once. `proxy` is the same JSON object as the body of POST /api/proxies: `name`, `ports` (the port ids from `list_ports`), `protocol` (`http`, `tcp` or `udp`) and the fields of that protocol on the same level, for example {\"name\": \"site\", \"ports\": [\"<port id>\"], \"protocol\": \"http\", \"vhosts\": [\"example.com\"], \"routes\": [{\"path\": \"/\", \"servers\": [{\"url\": \"http://127.0.0.1:8080/\"}]}]}; a TCP proxy takes {\"protocol\": \"tcp\", \"upstream_servers\": [{\"addr\": \"/dns/db.internal/tcp/5432\"}]}. Returns the name of the new proxy; read its id with `list_proxies`. Needs a token with the edit scope and an editor or admin account."
    )]
    async fn create_proxy(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddProxyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let proxy: Proxy = params.proxy;
        let name = proxy.name.clone();
        let owner = caller.proxies.is_some().then(|| caller.username.clone());
        self.state
            .call(
                &caller,
                AddProxy {
                    entry: proxy,
                    owner,
                },
            )
            .await
            .map_err(tool_error)?;
        json_result(json!({ "created": name }))
    }

    /// An update of a proxy from the JSON form of the admin API.
    #[tool(
        description = "Replace the whole configuration of a proxy. `entry` is the proxy object with its `id`, as `get_proxy` returns it, with your changes applied; a field you leave out falls back to its default and is lost, so always start from `get_proxy`. A basic auth user or a bearer token sent without a new secret keeps its stored hash. Applies at once. A proxy that discovery (Docker labels, etcd) created is read-only and is refused with `proxy_read_only`. Needs a token with the edit scope and an editor or admin account; an editor with a proxy list changes only the proxies on its list."
    )]
    async fn update_proxy(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateProxyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let entry: ProxyEntry = params.entry;
        self.state
            .call(&caller, UpdateProxy { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "updated": true }))
    }

    /// A deletion of a proxy.
    #[tool(
        description = "Delete a proxy by its `id`; its sites stop being served at once. A proxy that discovery (Docker labels, etcd) created is refused with `proxy_read_only`. Needs a token with the edit scope and an editor or admin account; an editor with a proxy list deletes only the proxies on its list."
    )]
    async fn delete_proxy(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        self.state
            .call(&caller, DeleteProxy { id })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }

    /// Purge cache for a proxy.
    #[tool(
        description = "Delete cached responses of one HTTP proxy (`id`) from the response cache, so the next requests go to the upstream servers. `path` limits the purge to URLs that start with that path prefix; without it the whole cache of the proxy is purged. Needs a token with the edit scope and an editor or admin account; an editor with a proxy list purges only the proxies on its list."
    )]
    async fn purge_proxy_cache(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<PurgeProxyCacheParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        self.state
            .call(
                &caller,
                PurgeProxyCache {
                    id,
                    path: params.path,
                },
            )
            .await
            .map_err(tool_error)?;
        json_result(json!({ "purged": true }))
    }

    /// List all compression dictionaries for an HTTP proxy.
    #[tool(
        description = "List the compression dictionaries of one HTTP proxy (`proxy_id`) with their id, name, URL match pattern, serving path, raw content and SHA-256 hash. A browser that fetched a dictionary sends its hash back, and the proxy then compresses matching responses against it (Compression Dictionary Transport). Fails for TCP and UDP proxies. Read-only; any account can call it."
    )]
    async fn list_compression_dictionaries(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ListCompressionDictionariesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let proxy_id: ShortId = parse_id(&params.proxy_id)?;
        let entry: ProxyEntry = *self
            .state
            .call(&caller, GetProxy { id: proxy_id })
            .await
            .map_err(tool_error)?;
        let ProxyKind::Http(ref http) = entry.proxy.kind else {
            return Err(ErrorData::invalid_params(
                "Proxy is not an HTTP proxy",
                None,
            ));
        };
        json_result(&http.compression.dictionaries)
    }

    /// Get a compression dictionary by proxy id and dictionary id.
    #[tool(
        description = "Show one compression dictionary of an HTTP proxy by `proxy_id` and `dict_id` (from `list_compression_dictionaries`), with its match pattern, serving path, raw content and SHA-256 hash. Read-only; any account can call it."
    )]
    async fn get_compression_dictionary(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ProxyDictIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let proxy_id: ShortId = parse_id(&params.proxy_id)?;
        let dict_id: ShortId = parse_id(&params.dict_id)?;
        let entry: ProxyEntry = *self
            .state
            .call(&caller, GetProxy { id: proxy_id })
            .await
            .map_err(tool_error)?;
        let ProxyKind::Http(ref http) = entry.proxy.kind else {
            return Err(ErrorData::invalid_params(
                "Proxy is not an HTTP proxy",
                None,
            ));
        };
        let dict = http
            .compression
            .dictionaries
            .iter()
            .find(|d| d.id == dict_id)
            .ok_or_else(|| {
                ErrorData::invalid_params(format!("Dictionary not found: {dict_id}"), None)
            })?;
        json_result(dict)
    }

    /// Add a compression dictionary to an HTTP proxy.
    #[tool(
        description = "Add a compression dictionary to one HTTP proxy (`proxy_id`). The proxy serves `raw_data` at `path` with a `Use-As-Dictionary: match=\"<match_pattern>\"` header, computes its SHA-256 hash itself, and compresses later responses whose URL matches `match_pattern` against it. Returns the new dictionary id and its hash. Needs a token with the edit scope and an editor or admin account; an editor with a proxy list changes only the proxies on its list."
    )]
    async fn create_compression_dictionary(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddCompressionDictionaryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let proxy_id: ShortId = parse_id(&params.proxy_id)?;
        let mut entry: ProxyEntry = *self
            .state
            .call(&caller, GetProxy { id: proxy_id })
            .await
            .map_err(tool_error)?;
        let ProxyKind::Http(ref mut http) = entry.proxy.kind else {
            return Err(ErrorData::invalid_params(
                "Proxy is not an HTTP proxy",
                None,
            ));
        };
        let mut dict = CompressionDictionary {
            id: ShortId::new(),
            name: params.name,
            match_pattern: params.match_pattern,
            path: params.path,
            raw_data: params.raw_data,
            sha256_sf: String::new(),
        };
        dict.ensure_hash();
        dict.validate().map_err(tool_error)?;
        http.compression.dictionaries.push(dict.clone());
        self.state
            .call(&caller, UpdateProxy { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({
            "id": dict.id.to_string(),
            "sha256_sf": dict.sha256_sf,
            "created": true
        }))
    }

    /// Delete a compression dictionary from an HTTP proxy.
    #[tool(
        description = "Delete one compression dictionary (`dict_id`) from an HTTP proxy (`proxy_id`); the proxy stops serving it and stops compressing against it. Needs a token with the edit scope and an editor or admin account; an editor with a proxy list changes only the proxies on its list."
    )]
    async fn delete_compression_dictionary(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ProxyDictIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let proxy_id: ShortId = parse_id(&params.proxy_id)?;
        let dict_id: ShortId = parse_id(&params.dict_id)?;
        let mut entry: ProxyEntry = *self
            .state
            .call(&caller, GetProxy { id: proxy_id })
            .await
            .map_err(tool_error)?;
        let ProxyKind::Http(ref mut http) = entry.proxy.kind else {
            return Err(ErrorData::invalid_params(
                "Proxy is not an HTTP proxy",
                None,
            ));
        };
        let initial_len = http.compression.dictionaries.len();
        http.compression.dictionaries.retain(|d| d.id != dict_id);
        if http.compression.dictionaries.len() == initial_len {
            return Err(ErrorData::invalid_params(
                format!("Dictionary not found: {dict_id}"),
                None,
            ));
        }
        self.state
            .call(&caller, UpdateProxy { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "deleted": dict_id.to_string() }))
    }

    // ==========================================
    // Group 4: Certificates & ACME
    // ==========================================

    /// The certificates.
    #[tool(
        description = "List every stored certificate with its id, kind (`server`, `client` or `root`), fingerprint, issuer, the domain names it covers (SAN) and its expiry time (`not_after`, Unix seconds). Certificates from ACME and uploads are both listed. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_certs(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let certs: Vec<r3v3rs3_api::cert::CertInfo> = *self
            .state
            .call(&caller, GetCertList)
            .await
            .map_err(tool_error)?;
        json_result(certs)
    }

    /// Details of one certificate.
    #[tool(
        description = "Show one certificate by its `id` (from `list_certs`): kind, fingerprint, issuer, covered domain names and expiry time. The private key is never returned. Read-only; any account can call it."
    )]
    async fn get_cert(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let cert = self
            .state
            .call(&caller, GetCert { id })
            .await
            .map_err(tool_error)?;
        json_result(cert.info())
    }

    /// Certificate domain coverage check.
    #[tool(
        description = "Check for each name in `domains` whether a stored certificate covers it, and whether the cover comes from a shared wildcard certificate of the parent domain (with that root domain). Use it before you add a site, to see whether it needs a new ACME request. Read-only; any account can call it."
    )]
    async fn check_cert_coverage(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CertCoverageParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let coverage = self
            .state
            .call(
                &caller,
                GetCertCoverage {
                    domains: params.domains,
                },
            )
            .await
            .map_err(tool_error)?;
        json_result(coverage)
    }

    /// Delete a certificate.
    #[tool(
        description = "Delete a certificate by its `id` with its private key. TLS ports stop offering it at once; a site that has no other covering certificate fails its TLS handshake. An ACME request renews into a new certificate on its own schedule. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_cert(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        self.state
            .call(&caller, DeleteCert { id })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }

    /// List ACME certificate orders.
    #[tool(
        description = "List every ACME request (automatic certificate) with its id, active flag, CA provider name, domain names, challenge type (`http-01`, `tls-alpn-01` or `dns-01`), DNS provider or DNS credential, CNAME alias, renewal period in days and the next renewal time. Whether an order succeeded shows in `list_certs`. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_acme(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let list: Vec<r3v3rs3_api::acme::AcmeInfo> = *self
            .state
            .call(&caller, GetAcmeList)
            .await
            .map_err(tool_error)?;
        json_result(list)
    }

    /// Get one ACME request.
    #[tool(
        description = "Show one ACME request by its `id` (from `list_acme`): active flag, CA provider name, domain names, challenge type, DNS provider name or DNS credential id, CNAME alias, renewal period and next renewal time. DNS provider credentials are never returned. Read-only; any account can call it."
    )]
    async fn get_acme(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let acme: r3v3rs3_api::acme::AcmeInfo = *self
            .state
            .call(&caller, GetAcme { id })
            .await
            .map_err(tool_error)?;
        json_result(acme)
    }

    /// Create an ACME certificate request.
    #[tool(
        description = "Create an ACME request that obtains a certificate and renews it automatically. `request` is the body of POST /api/acme: `server_url` (the CA directory, for example Let's Encrypt), `contacts`, `identifiers` (the domain names), `challenge_type` (`http-01` needs port 80, `tls-alpn-01` needs port 443, `dns-01` is required for wildcards and needs `dns_credential` from `list_dns_credentials` or an inline `dns_provider`), optional `dns_alias` and `eab`, and `renewal_days`. The first order starts in the background; follow it with `list_acme` and `list_certs`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_acme(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddAcmeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::acme::AcmeRequest = params.request;
        self.state
            .call(&caller, AddAcme { request })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "created": true }))
    }

    /// Delete an ACME certificate request.
    #[tool(
        description = "Delete an ACME request by its `id`; its certificate is no longer renewed. Certificates it already issued stay until you delete them with `delete_cert`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_acme(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        self.state
            .call(&caller, DeleteAcme { id })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }

    /// Trigger immediate ACME order/renewal.
    #[tool(
        description = "Start an order of the ACME request `id` now instead of waiting for its renewal time, for example after you fixed DNS or a firewall. The order runs in the background; check the result with `list_acme` and `list_certs`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn request_acme(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        self.state
            .call(&caller, RequestAcme { id })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "requested": id.to_string() }))
    }

    /// Get CNAME delegation instructions and verification status for an ACME request.
    #[tool(
        description = "For a `dns-01` ACME request with a `dns_alias` (`id`), list per domain the CNAME record to create (`_acme-challenge.<domain>` pointing at the alias name), whether public DNS already shows it, and a message. Use it to set up DNS delegation before `request_acme`. Read-only; any account can call it."
    )]
    async fn get_acme_cname_instructions(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let instructions: Vec<r3v3rs3_api::acme::AcmeCnameInstruction> = *self
            .state
            .call(&caller, GetAcmeCnameInstructions { id })
            .await
            .map_err(tool_error)?;
        json_result(instructions)
    }

    /// List configured DNS provider credentials.
    #[tool(
        description = "List the stored DNS provider credentials that `dns-01` ACME requests reference, with their id, name and provider (for example `cloudflare`). The API keys themselves are never returned. Pass an id from here as `dns_credential` in `create_acme`. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_dns_credentials(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let list = self
            .state
            .call(&caller, GetDnsCredentialList)
            .await
            .map_err(tool_error)?;
        json_result(list)
    }

    // ==========================================
    // Group 5: Access Lists
    // ==========================================

    /// List all access control lists.
    #[tool(
        description = "List every access list with its id, name, IP filter (`allow` and `deny` CIDR blocks, `allow_countries` and `deny_countries` ISO codes; deny wins) and authentication policy. An access list is a reusable rule set that HTTP proxies and routes reference by id through their `access_list` field. Secrets of its users and tokens are not returned. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_access_lists(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let lists: Vec<r3v3rs3_api::access_list::AccessListEntry> = *self
            .state
            .call(&caller, GetAccessLists)
            .await
            .map_err(tool_error)?;
        json_result(lists)
    }

    /// Create an access control list.
    #[tool(
        description = "Create an access list. `list` is the body of POST /api/access_lists: `name`, `ip_filter` and `auth`. Reference the new list from an HTTP proxy or route with its `access_list` field (read the id with `list_access_lists`). Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_access_list(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddAccessListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let list: r3v3rs3_api::access_list::AccessList = params.list;
        self.state
            .call(&caller, AddAccessList { list })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "created": true }))
    }

    /// Update an access control list.
    #[tool(
        description = "Replace the whole content of an access list. `entry` is the list with its `id` and new `name`, `ip_filter` and `auth`; start from `list_access_lists`, because omitted fields fall back to their defaults. A user or a token sent without a new secret keeps its stored hash. Every proxy that references the list uses the new rules at once. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_access_list(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateAccessListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let entry: r3v3rs3_api::access_list::AccessListEntry = params.entry;
        self.state
            .call(&caller, UpdateAccessList { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "updated": true }))
    }

    /// Delete an access control list.
    #[tool(
        description = "Delete an access list by its `id`. It is refused with `access_list_in_use` while a proxy or route references it; remove the reference first. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_access_list(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        self.state
            .call(&caller, DeleteAccessList { id })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }

    /// Read one named DNS credential.
    #[tool(
        description = "Show one named DNS credential (`id` from `list_dns_credentials`): its name, provider and which fields are set. Secrets such as API tokens are never returned. Read-only; any account can call it."
    )]
    async fn get_dns_credential(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let info = super::dns_credentials::get(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(info.0)
    }

    /// Create a named DNS credential.
    #[tool(
        description = "Create a named DNS credential that ACME `dns-01` requests reference by id. `request` is the body of POST /api/dns_credentials: `name` and `dns_provider` (for example {\"provider\": \"cloudflare\", \"api_token\": \"...\"}). The secret is sealed and never returned. Needs a token with the edit scope and an admin account."
    )]
    async fn create_dns_credential(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DnsCredentialParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let info = super::dns_credentials::add(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(params.request),
        )
        .await
        .map_err(any_error)?;
        json_result(info.0)
    }

    /// Rename a named DNS credential or replace its secrets.
    #[tool(
        description = "Change a named DNS credential (`id`). `request` holds the new `name` and, only when the secrets change, a new `dns_provider`; without `dns_provider` the stored secrets stay. Needs a token with the edit scope and an admin account."
    )]
    async fn update_dns_credential(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateDnsCredentialParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let _ = super::dns_credentials::put(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(params.request),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "updated": id.to_string() }))
    }

    /// Delete a named DNS credential.
    #[tool(
        description = "Delete a named DNS credential (`id`). It is refused while an ACME request references it. Needs a token with the edit scope and an admin account."
    )]
    async fn delete_dns_credential(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let _ = super::dns_credentials::delete(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }

    // ==========================================
    // Group 5b: Error Page Themes & Dynamic Variables
    // ==========================================

    /// List all error page themes.
    #[tool(
        description = "List every error page theme with its id, name, description and, for each custom page, its HTTP status code and the size of its body in bytes. The HTML of the pages is left out, because a built-in theme holds tens of KB of it; read it with `get_error_page_theme`. An HTTP proxy shows the pages of the theme set in its `error_page_theme` field instead of the built-in error pages. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_error_page_themes(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let themes: Vec<r3v3rs3_api::error_page_theme::ErrorPageTheme> = *self
            .state
            .call(
                &caller,
                crate::server::rpc::error_page_themes::GetErrorPageThemes,
            )
            .await
            .map_err(tool_error)?;
        json_result(themes.iter().map(theme_summary).collect::<Vec<_>>())
    }

    /// Get an error page theme by id.
    #[tool(
        description = "Show one error page theme by its `id`: name, description and every page with its HTTP status code and HTML body. Read-only; any account can call it."
    )]
    async fn get_error_page_theme(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let theme: r3v3rs3_api::error_page_theme::ErrorPageTheme = *self
            .state
            .call(
                &caller,
                crate::server::rpc::error_page_themes::GetErrorPageTheme { id },
            )
            .await
            .map_err(tool_error)?;
        json_result(theme)
    }

    /// Add a new error page theme.
    #[tool(
        description = "Create an error page theme from `name`, an optional `description` and `pages`, a list of {\"status\": <HTTP status code>, \"body\": \"<HTML>\"}. Returns the id of the new theme; set it as `error_page_theme` of an HTTP proxy to use it. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_error_page_theme(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddErrorPageThemeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let pages = params.pages.unwrap_or_default();
        let theme = r3v3rs3_api::error_page_theme::ErrorPageTheme {
            id: ShortId::new(),
            name: params.name,
            description: params.description.unwrap_or_default(),
            pages,
        };
        let id: ShortId = *self
            .state
            .call(
                &caller,
                crate::server::rpc::error_page_themes::AddErrorPageTheme { theme },
            )
            .await
            .map_err(tool_error)?;
        json_result(json!({ "id": id.to_string(), "created": true }))
    }

    /// Update an existing error page theme.
    #[tool(
        description = "Change an error page theme by its `id`. Pass only the fields to change: `name`, `description`, or `pages` (which replaces the whole page list). Proxies that use the theme show the new pages at once. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_error_page_theme(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateErrorPageThemeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let mut theme = (*self
            .state
            .call(
                &caller,
                crate::server::rpc::error_page_themes::GetErrorPageTheme { id },
            )
            .await
            .map_err(tool_error)?)
        .clone();
        if let Some(name) = params.name {
            theme.name = name;
        }
        if let Some(desc) = params.description {
            theme.description = desc;
        }
        if let Some(pages_val) = params.pages {
            theme.pages = pages_val;
        }
        self.state
            .call(
                &caller,
                crate::server::rpc::error_page_themes::UpdateErrorPageTheme { theme },
            )
            .await
            .map_err(tool_error)?;
        json_result(json!({ "id": id.to_string(), "updated": true }))
    }

    /// Delete an error page theme.
    #[tool(
        description = "Delete an error page theme by its `id`. It is refused with `error_page_theme_in_use` while an HTTP proxy uses the theme; remove it from those proxies first. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_error_page_theme(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        self.state
            .call(
                &caller,
                crate::server::rpc::error_page_themes::DeleteErrorPageTheme { id },
            )
            .await
            .map_err(tool_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }

    /// Reset error page themes to default templates.
    #[tool(
        description = "Replace all error page themes with the built-in default themes and return them. Every custom theme and every change to a built-in theme is lost. Takes no arguments. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn reset_error_page_themes(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let themes: Vec<r3v3rs3_api::error_page_theme::ErrorPageTheme> = *self
            .state
            .call(
                &caller,
                crate::server::rpc::error_page_themes::ResetErrorPageThemes,
            )
            .await
            .map_err(tool_error)?;
        json_result(themes)
    }

    // ==========================================
    // Group 6: Platform Applications (PaaS)
    // ==========================================

    /// List all PaaS applications. Database servers are listed by `list_databases`.
    #[tool(
        description = "List every app of the deployment platform with its id, name, target, spec (image, Git or Compose source, domains, ports), the image update status, the Git push webhook and, for App Store apps, whether the catalog has a newer version. The state of the deployments comes from `list_app_deployments`. Managed databases are not included; `list_databases` lists them. Takes no arguments. Read-only; any account without a proxy list. The platform must be enabled in the settings."
    )]
    async fn list_apps(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let mut apps =
            super::platform::without_databases(platform.apps().await.map_err(any_error)?);
        let catalog = crate::appstore::state().catalog.clone();
        for app in &mut apps {
            app.catalog_update = super::platform::check_catalog_update(app, &catalog);
        }
        json_result(apps)
    }

    /// Get detailed configuration and status of an application.
    #[tool(
        description = "Show one app by its `id` (from `list_apps`): name, target, the full spec, the image update status, the Git push webhook and whether a Git token and a webhook secret are set (never their values), plus the catalog update status of App Store apps. Use the output as the starting point of `update_app`, because that tool replaces the whole spec. Read-only; any account without a proxy list."
    )]
    async fn get_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let mut app = platform.app(id).await.map_err(any_error)?;
        let catalog = crate::appstore::state().catalog.clone();
        app.catalog_update = super::platform::check_catalog_update(&app, &catalog);
        json_result(app)
    }

    /// Create a new PaaS application.
    #[tool(
        description = "Create an app on the deployment platform without starting it. `app` is the body of POST /api/apps: `name`, `target` (`local` for the Docker engine of this server, or an agent target id from `list_targets`) and `spec` with `source` ({\"type\": \"image\", \"image\": \"nginx:1.27\"}, {\"type\": \"git\", \"repository\": \"https://...\", \"branch\": \"main\"} or {\"type\": \"compose\", ...}), the container `port`, `domains` and the listener `ports` that route to it. Returns the new app with its id. Set environment variables with `update_app_env`, then start it with `deploy_app`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddAppParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let app = super::platform::add_app(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(params.app),
        )
        .await
        .map_err(any_error)?;
        json_result(app.0)
    }

    /// Update an application's specification.
    #[tool(
        description = "Replace the name, the target and the whole spec of an app (`id`, `app` in the form of POST /api/apps). Start from `get_app`, because fields you leave out fall back to their defaults. The running containers change only at the next deployment; call `deploy_app` to apply the change. Returns the updated app. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateAppParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let app = super::platform::update_app(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(params.app),
        )
        .await
        .map_err(any_error)?;
        json_result(app.0)
    }

    /// Delete an application.
    #[tool(
        description = "Delete an app by its `id` with its containers on every target, its environment variables, its deployment history and its Git push webhook; its domains stop being served. A database that apps still connect to is refused. This cannot be undone. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let axum::Json(()) = super::platform::delete_app(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }

    /// Trigger immediate application deployment.
    #[tool(
        description = "Start a deployment of the current spec and environment of an app (`id`): pull the image, build it from Git or run `docker compose`, start the new containers and switch the traffic over (blue-green for image apps). It returns the queued deployment at once; follow it with `list_app_deployments` and `get_app_logs`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn deploy_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let deployment = super::platform::deploy_app(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(deployment.0)
    }

    /// Retrieve container execution logs.
    #[tool(
        description = "Return the last lines of the container log of an app (`id`). `tail` sets the number of lines (1 to 1000, default 200); `service` selects one service of a Compose app. Use it to find why a deployment or a container fails. Read-only; any account without a proxy list."
    )]
    async fn get_app_logs(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AppLogsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let tail = params.tail.unwrap_or(crate::platform::DEFAULT_LOG_TAIL);
        let logs = platform
            .app_log(id, tail, params.service.as_deref())
            .await
            .map_err(any_error)?;
        json_result(logs)
    }

    /// Inspect container log file sizes and overview for an application.
    #[tool(
        description = "Show the total disk size of the container log files of an app (`id`) and the size per container, to decide whether `vacuum_app_logs` is needed. Read-only; any account without a proxy list."
    )]
    async fn get_app_log_size(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let overview = platform.app_log_size(id).await.map_err(any_error)?;
        json_result(overview)
    }

    /// Get container resource throttling, CFS quota, and OOM diagnostic status for an application.
    #[tool(
        description = "Show live resource diagnostics of an app (`id`): whether it runs, a health verdict, CPU use and how much of the time the CPU quota throttled it, memory use against the memory limit with the failed allocation count, and whether the kernel killed it for running out of memory (OOM). Use it when an app is slow or restarts. Read-only; any account without a proxy list."
    )]
    async fn get_app_resource_diagnostics(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let diagnostics = platform
            .app_resource_diagnostics(id)
            .await
            .map_err(any_error)?;
        json_result(diagnostics)
    }

    /// Get unified dashboard summary for an application.
    #[tool(
        description = "Return everything about one app (`id`) in one call, as the app page of the panel shows it: the app, its latest and its running deployment, the state of its Compose services, resource diagnostics, metric points, a preview of the latest log lines, the log size and the uptime. Use it as the first look at an app. Read-only; any account without a proxy list."
    )]
    async fn get_app_dashboard(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let summary = platform.app_dashboard(id).await.map_err(any_error)?;
        json_result(summary)
    }

    /// Delete the rotated container log files of an application.
    #[tool(
        description = "Delete the rotated container log files (`<id>-json.log.N`) of an app (`id`) to free disk space at once, and return the number of deleted files and bytes freed. The active log file stays, because Docker Engine stops serving `docker logs --tail` for a file emptied from outside; `max-size` caps it. The deleted lines are lost. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn vacuum_app_logs(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let res = platform.vacuum_app_logs(id).await.map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::VacuumLogs)
            .id(id)
            .summary(format!(
                "Vacuumed container logs, freed {} bytes",
                res.freed_bytes
            ));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(res)
    }

    /// Get environment variables of an application.
    #[tool(
        description = "List the environment variables of an app (`id`) as {key, value, secret}. A secret variable is returned without its value. Read-only, but it needs an admin account or an editor without a proxy list."
    )]
    async fn get_app_env(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let env = platform.env(id).await.map_err(any_error)?;
        json_result(env)
    }

    /// Update environment variables of an application.
    #[tool(
        description = "Replace the whole list of environment variables of an app (`id`) with `entries`, a list of {\"key\": \"FOO\", \"value\": \"bar\", \"secret\": false}. A variable missing from the list is deleted, so start from `get_app_env`. A secret variable sent without a value keeps its stored value. The containers get the new values only at the next deployment; call `deploy_app`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_app_env(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateAppEnvParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let axum::Json(()) = super::platform::put_env(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(params.entries),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "updated": true }))
    }

    /// Generate cryptographically secure secrets for missing or placeholder environment variables of an application.
    #[tool(
        description = "Fill the empty or placeholder environment variables of an app (`id`) with random secrets (for example passwords and keys that a Compose template asks for). `save` (default true) stores them on the app; `overwrite` (default false) also replaces variables that already have a value. Returns the generated keys. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn generate_app_env_secrets(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<GenerateAppEnvSecretsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let request = r3v3rs3_api::compose_env::GenerateAppEnvSecretsRequest {
            save: params.save.unwrap_or(true),
            overwrite: params.overwrite.unwrap_or(false),
        };
        let generated = super::platform::generate_app_env(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(generated.0)
    }

    /// Resolve an environment variable template (.env text or entries) and generate cryptographically secure secrets.
    #[tool(
        description = "Fill the empty or placeholder values of an environment template with random secrets and return the result; nothing is stored. Give either `template` (the text of a .env file; the result comes back as text and as entries) or `entries` (a list of {key, value, secret}). `overwrite` (default false) also replaces values that are already set. Returns the filled template or entries and the names of the generated keys. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn resolve_compose_env_template(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ResolveComposeEnvParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let _ = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let overwrite = params.overwrite.unwrap_or(false);
        if let Some(template) = params.template {
            let (resolved, entries, generated_keys) =
                r3v3rs3_api::compose_env::resolve_env_template(
                    &template,
                    overwrite,
                    super::platform::generate_secret_for_key,
                );
            return json_result(r3v3rs3_api::compose_env::ResolveEnvTemplateResponse {
                template: resolved,
                generated_count: generated_keys.len(),
                generated_keys,
                entries,
            });
        }
        let mut entries: Vec<r3v3rs3_api::platform::EnvEntry> = params.entries.unwrap_or_default();
        let generated_keys = r3v3rs3_api::compose_env::resolve_env_entries(
            &mut entries,
            overwrite,
            super::platform::generate_secret_for_key,
        );
        json_result(r3v3rs3_api::compose_env::ResolveEnvTemplateResponse {
            template: String::new(),
            generated_count: generated_keys.len(),
            generated_keys,
            entries,
        })
    }

    /// List deployment history of an application.
    #[tool(
        description = "List the deployments of an app (`id`), newest first, with each deployment id, trigger, state (`queued`, `pending_approval`, `building`, `deploying`, `running`, `superseded`, `failed` or `cancelled`), image digest and times. Use a deployment id from here for `rollback_deployment` or `cancel_deployment`. Read-only; any account without a proxy list."
    )]
    async fn list_app_deployments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let deployments = platform.deployments(id).await.map_err(any_error)?;
        json_result(deployments)
    }

    /// Roll back an application to a prior deployment.
    #[tool(
        description = "Start a new deployment that repeats an earlier one (`deployment_id` from `list_app_deployments`): the same image digest, spec and environment. Use it to go back to the last working version. Returns the queued deployment. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn rollback_deployment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<RollbackParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let dep_id: ShortId = parse_id(&params.deployment_id)?;
        let deployment = super::platform::rollback_deployment(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(dep_id),
        )
        .await
        .map_err(any_error)?;
        json_result(deployment.0)
    }

    /// Cancel an in-progress or pending deployment.
    #[tool(
        description = "Cancel a queued or running deployment (`deployment_id`). The containers of the previous deployment keep serving. Returns the cancelled deployment. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn cancel_deployment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<RollbackParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let dep_id: ShortId = parse_id(&params.deployment_id)?;
        let deployment = super::platform::cancel_deployment(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(dep_id),
        )
        .await
        .map_err(any_error)?;
        json_result(deployment.0)
    }

    /// Check registry for updated container image.
    #[tool(
        description = "Ask the container registry whether the image tag of an image app (`id`) points to a newer digest than the running one. When the app has auto-deploy on and an update is found, a deployment starts. Returns the result of the check. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn check_app_update(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let status = super::platform::check_app_update(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(status.0)
    }

    /// List scheduled commands and cron tasks configured for an application.
    #[tool(
        description = "List the scheduled commands of an app (`id`) with their id, name, cron schedule, shell command, enabled flag, and the time, status and output of the last run. Read-only; any account without a proxy list."
    )]
    async fn list_app_scheduled_commands(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let list = platform.scheduled_commands(id).await.map_err(any_error)?;
        json_result(list)
    }

    /// Add a new scheduled cron command to an application.
    #[tool(
        description = "Add a command that runs on a schedule inside the running container of an app: `app_id`, `name`, `schedule` (cron syntax, for example `0 3 * * *` or `@daily`), `command` (a shell command) and `enabled` (default true). Returns the new command with its id. Needs a token with the edit scope and an admin account."
    )]
    async fn create_app_scheduled_command(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddAppScheduledCommandParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.app_id)?;
        let request = r3v3rs3_api::platform::ScheduledCommandRequest {
            name: params.name,
            schedule: params.schedule,
            command: params.command,
            enabled: params.enabled.unwrap_or(true),
        };
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let cmd = platform
            .add_scheduled_command(id, request)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::AddScheduledCommand)
            .id(id)
            .summary(&cmd.name);
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(cmd)
    }

    /// Update an existing scheduled cron command of an application.
    #[tool(
        description = "Replace a scheduled command (`app_id`, `command_id`) with a new `name`, `schedule`, `command` and `enabled`; all of `name`, `schedule` and `command` are required, and an omitted `enabled` turns the command on. Returns the updated command. Needs a token with the edit scope and an admin account."
    )]
    async fn update_app_scheduled_command(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateAppScheduledCommandParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.app_id)?;
        let cmd_id: ShortId = parse_id(&params.command_id)?;
        let request = r3v3rs3_api::platform::ScheduledCommandRequest {
            name: params.name,
            schedule: params.schedule,
            command: params.command,
            enabled: params.enabled.unwrap_or(true),
        };
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let cmd = platform
            .update_scheduled_command(id, cmd_id, request)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::UpdateScheduledCommand)
            .id(id)
            .summary(&cmd.name);
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(cmd)
    }

    /// Delete a scheduled command from an application.
    #[tool(
        description = "Delete a scheduled command (`app_id`, `command_id`); it no longer runs. Needs a token with the edit scope and an admin account."
    )]
    async fn delete_app_scheduled_command(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AppCommandTargetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.app_id)?;
        let cmd_id: ShortId = parse_id(&params.command_id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        platform
            .delete_scheduled_command(id, cmd_id)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::DeleteScheduledCommand)
            .id(id)
            .summary(cmd_id.to_string());
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(json!({ "deleted": true }))
    }

    /// Run a scheduled command immediately inside the running container of an application.
    #[tool(
        description = "Run a scheduled command (`app_id`, `command_id`) once, now, inside the running container of the app, and return whether it succeeded, its status, its output and its duration. Its schedule does not change. Needs a token with the edit scope and an admin account."
    )]
    async fn run_app_scheduled_command(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AppCommandTargetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.app_id)?;
        let cmd_id: ShortId = parse_id(&params.command_id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let result = platform
            .execute_scheduled_command(id, cmd_id)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::RunScheduledCommand)
            .id(id)
            .summary(cmd_id.to_string());
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(result)
    }

    // ==========================================
    // Group 7: App Store Catalog
    // ==========================================

    /// View the App Store catalog.
    #[tool(
        description = "Show the App Store catalog: every one-click app with its catalog id, name, description, category, image or Compose source, container port and the environment template a Compose app asks for. Pass a catalog id to `install_store_app`. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_store(
        &self,
        Extension(_parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let response = crate::appstore::response();
        json_result(response)
    }

    /// Refresh the App Store catalog.
    #[tool(
        description = "Download the App Store catalog from the r3v3rs3-apps repository now and install it, instead of waiting for the daily refresh. Takes no arguments. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn refresh_store(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        // The admin handler checks the role and writes the audit record; the tool answers with a
        // short confirmation instead of the whole catalog.
        let _catalog = super::store::refresh(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "refreshed": true }))
    }

    /// Install and deploy an application directly from the App Store.
    #[tool(
        description = "Create an app from an App Store entry and start its first deployment in one step. `app_id` is the catalog id from `get_store`; `name` defaults to the catalog id, `target` to `local`; `domains` and `ports` (listener port names or ids) set the routing. A Compose app whose environment template needs values should instead be created, given its variables with `update_app_env` or `generate_app_env_secrets`, and then deployed. Returns the app and the queued deployment. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn install_store_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<InstallStoreAppParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let catalog = crate::appstore::state().catalog.clone();
        let store_app = catalog
            .apps
            .iter()
            .find(|a| a.id == params.app_id)
            .cloned()
            .ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("app {} not found in store catalog", params.app_id),
                    None,
                )
            })?;
        let request = build_store_app_request(&store_app, &params)?;
        let axum::Json(app) = super::platform::add_app(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller.clone()),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        let axum::Json(deployment) = super::platform::deploy_app(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(app.id),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({
            "installed": true,
            "app": app,
            "deployment": deployment,
        }))
    }

    // ==========================================
    // Group 8: Managed Databases & Backups
    // ==========================================

    /// List all managed database applications.
    #[tool(
        description = "List every managed database (a database server app of the `postgres`, `mariadb`, `mysql`, `mongodb` or `redis` engine) with its id, name, target, spec (engine image, host port, volume) and image update status. Use the ids with the other database and backup tools. Takes no arguments. Read-only; any account without a proxy list. The platform must be enabled in the settings."
    )]
    async fn list_databases(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let apps = platform.apps().await.map_err(any_error)?;
        json_result(super::databases::databases_of(apps))
    }

    /// List available database engines.
    #[tool(
        description = "List the database engines that `create_database` accepts (`postgres`, `mariadb`, `mysql`, `mongodb`, `redis` in the shipped catalog) with their label, container port, default user and database, and the version tags to choose from. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_database_engines(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let _caller = caller_of(&parts)?;
        let engines = r3v3rs3_api::platform::engines();
        json_result(engines)
    }

    /// Create and deploy a new managed database.
    #[tool(
        description = "Create a managed database and deploy it at once: a container of the chosen engine with a data volume, a fixed host port and a generated password. `request` holds `name`, `engine` and `version` (from `list_database_engines`), an optional `host_port`, `limits`, and `project_id`/`environment_id`. Returns the app, the host port, the user, the database name and the password; the password appears only in this answer (later only `get_database_credentials` shows it). Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_database(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateDatabaseParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request = params.request;
        let created = super::databases::create(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(created.0)
    }

    /// Read the passwords of the accounts of a database.
    #[tool(
        description = "Return the passwords of a managed database (`id`): the root account and every tenant, each with user, password and database name. The audit log records every call, without the passwords. Needs a token with the edit scope and an admin account."
    )]
    async fn get_database_credentials(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id = parse_id(&params.id)?;
        let credentials = super::databases::credentials(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(credentials.0)
    }

    /// List the tenants of a shared database server.
    #[tool(
        description = "List the tenants of a shared database server (`id`; a database without a project): one database and user per project environment that uses the server, with the ids of the apps that connect as each tenant. Passwords are not returned. Read-only; any account without a proxy list."
    )]
    async fn list_database_tenants(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id = parse_id(&params.id)?;
        let tenants = super::databases::tenants(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(tenants.0)
    }

    /// Give the root account or a tenant of a database a new password.
    #[tool(
        description = "Give the root account (omit `tenant`) or one tenant of a managed database (`id`) a new random password, and start a new deployment of every app that connects with that account, so they get the new password. Returns the new password; it appears only in this answer. Needs a token with the edit scope and an admin account."
    )]
    async fn reset_database_password(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ResetDatabasePasswordParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id = parse_id(&params.id)?;
        let request = r3v3rs3_api::platform::DatabasePasswordRequest {
            tenant: params.tenant,
        };
        let reset = super::databases::reset_password(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(reset.0)
    }

    /// Remove a tenant of a shared database server with its data.
    #[tool(
        description = "Remove one tenant (`tenant`, as `list_database_tenants` names it) of a shared database server (`id`) with its user, its database and all its data. A tenant that an app still connects as is refused. This cannot be undone. Needs a token with the edit scope and an admin account."
    )]
    async fn delete_database_tenant(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DeleteDatabaseTenantParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id = parse_id(&params.id)?;
        let axum::Json(()) = super::databases::delete_tenant(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path((id, params.tenant.clone())),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "deleted": params.tenant }))
    }

    /// List database backup snapshots.
    #[tool(
        description = "List the backups of a managed database (`id`), newest first, with each backup id, engine, size, creation time and the result of its last restore test. Read-only; any account without a proxy list."
    )]
    async fn list_database_backups(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let backups = platform.list_backups(id).await.map_err(any_error)?;
        json_result(backups)
    }

    /// List the backups of every database.
    #[tool(
        description = "List the backups of every managed database with their database, size and creation time. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_all_backups(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let list = super::backups::list_all(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(list.0)
    }

    /// Summarize the backups of every database.
    #[tool(
        description = "Summarize the database backups: counts, total size, the newest backup and the databases without one. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_backups_overview(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let overview = super::backups::overview(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(overview.0)
    }

    /// Run a scheduled-style backup of one database now.
    #[tool(
        description = "Run a backup of the database `id` now, with the retention and S3 replication of the backup configuration, and return the new backup entry. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn run_database_backup_now(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request = r3v3rs3_api::platform::RunBackupRequest {
            database_id: parse_id(&params.id)?,
        };
        let entry = super::backups::run_backup_now(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(entry.0)
    }

    /// Restore a database from one of its backups.
    #[tool(
        description = "Restore the database `db_id` from its backup `backup_id` (from `list_database_backups`). The current data of the database is replaced. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn restore_database_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DeleteBackupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.db_id)?;
        let _ = super::backups::restore(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path((id, params.backup_id.clone())),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "restored": params.backup_id }))
    }

    /// List the backups stored in S3.
    #[tool(
        description = "List the database backup snapshots in the S3 bucket of the backup configuration, optionally only those of the database name `database`. Fails with `s3_not_configured` when no S3 storage is set. Read-only; any account can call it."
    )]
    async fn list_s3_database_backups(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<S3ListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let query = r3v3rs3_api::platform::S3SnapshotsQuery {
            database: params.database,
        };
        let list = super::backups::list_s3(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Query(query),
        )
        .await
        .map_err(any_error)?;
        json_result(list.0)
    }

    /// Restore a database from an S3 snapshot.
    #[tool(
        description = "Restore a database from an S3 snapshot. `request` is the body of POST /api/databases/s3/restore: `key` (from `list_s3_database_backups`) and either `target_db_id` (restore into that database) or `new_database_name` with an optional `engine` (create a new database). Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn restore_s3_database_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::platform::RestoreFromS3Request = json_arg(params.request)?;
        let done = super::backups::restore_s3(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(done.0)
    }

    /// List the read replicas of a database.
    #[tool(
        description = "List the read replicas of the database `id`. Read-only; any account can call it."
    )]
    async fn list_database_replicas(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let list = super::databases::list_replicas(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(list.0)
    }

    /// Create a read replica of a database.
    #[tool(
        description = "Create a read replica of the database `id`. `request` is the body of POST /api/databases/{id}/replicas: `name`, optional `host_port` and `limits`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_database_replica(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdJsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let request: r3v3rs3_api::platform::DatabaseReplicaRequest = json_arg(params.request)?;
        let created = super::databases::create_replica(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(created.0)
    }

    /// Clone a database into a new one.
    #[tool(
        description = "Clone the database `id` with its data into a new database. `request` is the body of POST /api/databases/{id}/clone: `name`, optional `host_port` and `limits`. The answer holds the new password once. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn clone_database(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdJsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let request: r3v3rs3_api::platform::DatabaseCloneRequest = json_arg(params.request)?;
        let created = super::databases::clone(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(created.0)
    }

    /// Delete a database.
    #[tool(
        description = "Delete the database `id` with its container. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_database(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let _ = super::databases::delete(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }

    /// Download the database engine catalog again.
    #[tool(
        description = "Download the database engine catalog again from the catalog repository and return the engines. Takes no arguments. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn refresh_database_engines(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let engines = super::databases::refresh_engines(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(engines.0)
    }

    /// Replace the settings of an ACME entry.
    #[tool(
        description = "Replace the settings of the ACME entry `id`. `request` is the body of PUT /api/acme/{id}: `active`, `renewal_days` and the other AcmeConfig fields. Needs a token with the edit scope and an account that may edit."
    )]
    async fn update_acme(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdJsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let config: r3v3rs3_api::acme::AcmeConfig = json_arg(params.request)?;
        let done = super::acme::put(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(config),
        )
        .await
        .map_err(any_error)?;
        json_result(done.0)
    }

    /// Create a self-signed certificate.
    #[tool(
        description = "Create a self-signed certificate. `request` holds `san` (the subject names), optional `ca_cert` (the id of the CA that signs it; without it a new CA is created and stored) and `kind` (`server` or `client`). Needs a token with the edit scope and an account that may edit."
    )]
    async fn create_self_signed_cert(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::cert::SelfSignedCertRequest = json_arg(params.request)?;
        let done = super::certs::self_sign(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(done.0)
    }

    /// Store an uploaded PEM certificate.
    #[tool(
        description = "Store a PEM certificate. `chain` is the PEM certificate chain, `key` the optional PEM private key, and `kind` one of `server` (default), `client` or `root`. Each value holds at most 1 MiB. Needs a token with the edit scope and an account that may edit."
    )]
    async fn upload_cert(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UploadCertParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let kind = match params.kind {
            Some(kind) => serde_json::from_value(serde_json::Value::String(kind))
                .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?,
            None => r3v3rs3_api::cert::CertKind::Server,
        };
        let key = params.key.map(String::into_bytes);
        let done =
            super::certs::add_pem(&self.state, &caller, kind, params.chain.into_bytes(), key)
                .await
                .map_err(any_error)?;
        json_result(done)
    }

    /// Read the PEM chain of a certificate.
    #[tool(
        description = "Return the PEM certificate chain of the certificate `id`. The private key is never returned; the CLI command `r3 cert download` saves the full archive. Read-only; any account can call it."
    )]
    async fn download_cert(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let pem = super::certs::chain_pem(&self.state, &caller, id)
            .await
            .map_err(any_error)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(pem)]))
    }

    /// Delete several certificates at once.
    #[tool(
        description = "Delete the certificates whose ids `request.ids` lists (at most 200) and report the result for each one. Needs a token with the edit scope and an account that may edit."
    )]
    async fn delete_certs(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::cert::DeleteCertsRequest = json_arg(params.request)?;
        let results = super::certs::delete_many(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(results.0)
    }

    /// Check the CNAME record of a domain.
    #[tool(
        description = "Check that `domain` has a CNAME record that points at `expected_target` (default: the host of the public URL of this panel). Returns `resolved_cname`, `is_valid` and a message. Read-only; any account can call it."
    )]
    async fn verify_cname(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<VerifyCnameParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let query = super::certs::CnameVerifyQuery {
            domain: params.domain,
            expected_target: params.expected_target,
        };
        let result = super::certs::verify_cname(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Query(query),
        )
        .await
        .map_err(any_error)?;
        json_result(result.0)
    }

    /// List the stored backups of an app volume.
    #[tool(
        description = "List the stored snapshot backups of the volume `volume` of the app `id`: backup id, size and time. Needs an admin account."
    )]
    async fn list_volume_backups(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<VolumeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let path = (parse_id(&params.id)?, params.volume);
        let entries = super::app_files::list_volume_backups(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(path),
        )
        .await
        .map_err(any_error)?;
        json_result(entries.0)
    }

    /// Back up an app volume now.
    #[tool(
        description = "Store a snapshot backup of the volume `volume` of the running app `id` and return it. The CLI command `r3 app volumes download-backup` saves the archive. Needs a token with the edit scope and an admin account."
    )]
    async fn create_volume_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<VolumeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let path = (parse_id(&params.id)?, params.volume);
        let entry = super::app_files::create_volume_backup(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(path),
        )
        .await
        .map_err(any_error)?;
        json_result(entry.0)
    }

    /// Restore an app volume from a backup.
    #[tool(
        description = "Replace the content of the volume `volume` of the app `id` with the stored backup `backup_id`. Needs a token with the edit scope and an admin account."
    )]
    async fn restore_volume_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<VolumeBackupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let path = (parse_id(&params.id)?, params.volume, params.backup_id);
        let axum::Json(()) = super::app_files::restore_volume_backup(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(path),
        )
        .await
        .map_err(any_error)?;
        json_result(serde_json::json!({ "restored": true }))
    }

    /// Delete a stored volume backup.
    #[tool(
        description = "Delete the stored backup `backup_id` of the volume `volume` of the app `id`. Needs a token with the edit scope and an admin account."
    )]
    async fn delete_volume_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<VolumeBackupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let path = (parse_id(&params.id)?, params.volume, params.backup_id);
        let axum::Json(()) = super::app_files::delete_volume_backup(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(path),
        )
        .await
        .map_err(any_error)?;
        json_result(serde_json::json!({ "deleted": true }))
    }

    /// List a directory inside a volume of an app.
    #[tool(
        description = "List the entries of the directory `path` inside a volume of the running app `id`: name, kind, size and modification time. Needs an admin account."
    )]
    async fn list_app_files(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AppFileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let (id, query) = params.query()?;
        let entries = super::app_files::list(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::extract::Query(query),
        )
        .await
        .map_err(any_error)?;
        json_result(entries.0)
    }

    /// Read a small text file inside a volume of an app.
    #[tool(
        description = "Return the content of the text file `path` inside a volume of the running app `id`. Only UTF-8 files up to 1 MiB are returned; the CLI command `r3 app files download` saves any file. Needs an admin account."
    )]
    async fn download_app_file(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AppFileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let (id, query) = params.query()?;
        let response = super::app_files::download(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::extract::Query(query),
        )
        .await
        .map_err(any_error)?;
        let response = axum::response::IntoResponse::into_response(response);
        let text = small_text(response, MAX_TOOL_FILE).await?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Write a small text file inside a volume of an app.
    #[tool(
        description = "Write `content` (UTF-8 text, at most 1 MiB) into the file `path` inside a writable volume of the running app `id`, replacing the file. The CLI command `r3 app files upload` writes any file. Needs a token with the edit scope and an admin account."
    )]
    async fn upload_app_file(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UploadAppFileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        if params.content.len() > MAX_TOOL_FILE {
            return Err(ErrorData::invalid_params(
                "content is larger than 1 MiB; use `r3 app files upload`",
                None,
            ));
        }
        let id: ShortId = parse_id(&params.id)?;
        let query = r3v3rs3_api::platform::FilePathQuery { path: params.path };
        let axum::Json(()) = super::app_files::upload(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::extract::Query(query),
            axum::body::Bytes::from(params.content),
        )
        .await
        .map_err(any_error)?;
        json_result(serde_json::json!({ "written": true }))
    }

    /// Delete a file or directory inside a volume of an app.
    #[tool(
        description = "Delete the file or directory `path` inside a writable volume of the running app `id`. Needs a token with the edit scope and an admin account."
    )]
    async fn delete_app_file(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AppFileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let (id, query) = params.query()?;
        let axum::Json(()) = super::app_files::delete(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::extract::Query(query),
        )
        .await
        .map_err(any_error)?;
        json_result(serde_json::json!({ "deleted": true }))
    }

    /// Seal every platform secret with a new master key.
    #[tool(
        description = "Create a new platform master key and seal every stored secret (app environments, Git connections, GitHub Apps, settings) with it. Returns the new key id and the count of resealed records. Needs a token with the edit scope and an admin account."
    )]
    async fn rotate_platform_key(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let report = super::platform::rotate_platform_key(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(report.0)
    }

    /// List the MCP sessions.
    #[tool(
        description = "List the MCP sessions that hold a valid token: session id, client id and name, account, scopes and times. Tokens are never returned. Needs an admin account."
    )]
    async fn list_mcp_sessions(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let sessions = super::oauth2::list_mcp_sessions(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(sessions.0)
    }

    /// End an MCP session.
    #[tool(
        description = "Revoke the MCP session `id` and its tokens; the client must authorize again. Needs a token with the edit scope and an admin account."
    )]
    async fn revoke_mcp_session(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        super::oauth2::revoke_mcp_session(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(params.id),
        )
        .await
        .map_err(any_error)?;
        json_result(serde_json::json!({ "revoked": true }))
    }

    /// Read the facts of this server.
    #[tool(
        description = "Show the server facts: version, build target and profile, configuration directory, whether the platform is on and whether a newer release is available. Read-only."
    )]
    async fn get_app_info(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        caller_of(&parts)?;
        let info = super::app_info::get(axum::extract::State(self.state.clone()))
            .await
            .map_err(any_error)?;
        json_result(info.0)
    }

    /// Read the Prometheus metrics.
    #[tool(
        description = "Return the metrics of the /api/metrics endpoint in the Prometheus text format: request, byte, status, latency and TLS handshake counters, limited to the proxies the account sees. Read-only."
    )]
    async fn get_metrics(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let response =
            super::metrics::metrics(axum::http::HeaderMap::new(), axum::Extension(caller))
                .await
                .map_err(any_error)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(
            response_text(response).await?,
        )]))
    }

    /// Read the Grafana dashboard.
    #[tool(
        description = "Return the Grafana dashboard JSON for the Prometheus metrics of this server, ready to import. Read-only."
    )]
    async fn get_grafana_dashboard(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let response = super::metrics::grafana_dashboard(axum::Extension(caller))
            .await
            .map_err(any_error)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(
            response_text(response).await?,
        )]))
    }

    /// Check the health of any gRPC server.
    #[tool(
        description = "Run grpc.health.v1.Health/Check against `request.target_url`, optionally for `request.service`. Needs a token with the edit scope and an account that may edit."
    )]
    async fn check_grpc_health(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::grpc::GrpcHealthCheckRequest = json_arg(params.request)?;
        let answer = super::grpc::health_standalone(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(answer.0)
    }

    /// Call a method of any gRPC server.
    #[tool(
        description = "Call the unary gRPC method `request.service`/`request.method` on `request.target_url` with the JSON `request.payload` and `request.metadata` headers; the server must offer reflection. Needs a token with the edit scope and an account that may edit."
    )]
    async fn invoke_grpc(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::grpc::GrpcInvokeRequest = json_arg(params.request)?;
        let answer = super::grpc::invoke_standalone(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(answer.0)
    }

    /// List the services of any gRPC server.
    #[tool(
        description = "List the services and methods of the gRPC server `request.target_url` through server reflection, optionally only `request.service_name`. Needs a token with the edit scope and an account that may edit."
    )]
    async fn reflect_grpc(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::grpc::GrpcReflectRequest = json_arg(params.request)?;
        let answer = super::grpc::reflect_standalone(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(answer.0)
    }

    /// Call a gRPC method through the upstream of a proxy.
    #[tool(
        description = "Call a unary gRPC method on the first upstream server of the proxy `id`. `request` holds `service`, `method`, `payload`, `metadata` and an optional `target_url`. Needs a token with the edit scope and an account that may edit proxies."
    )]
    async fn invoke_proxy_grpc(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdJsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let request: r3v3rs3_api::grpc::GrpcInvokeRequest = json_arg(params.request)?;
        let answer = super::grpc::invoke_proxy(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(answer.0)
    }

    /// List the gRPC services behind a proxy.
    #[tool(
        description = "List the services and methods of the first upstream server of the proxy `id` through gRPC server reflection; `request` may hold `service_name`. Needs a token with the edit scope."
    )]
    async fn reflect_proxy_grpc(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdJsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let request: r3v3rs3_api::grpc::GrpcReflectRequest = json_arg(params.request)?;
        let answer = super::grpc::reflect_proxy(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(Some(request)),
        )
        .await
        .map_err(any_error)?;
        json_result(answer.0)
    }

    /// Read the CDN IP range status.
    #[tool(
        description = "Show the CDN IP ranges that restore the client address: when and from where they were loaded, each provider with its range count, and the last download errors. Read-only."
    )]
    async fn get_cdn_status(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        caller_of(&parts)?;
        json_result(super::cdn::get().await.0)
    }

    /// Download the CDN IP ranges again.
    #[tool(
        description = "Download the CDN IP ranges from every provider now and apply them. Needs a token with the edit scope and an account that may edit."
    )]
    async fn refresh_cdn_ranges(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let status = super::cdn::refresh(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(status.0)
    }

    /// Read the state of the discovery sources.
    #[tool(
        description = "List the service discovery sources (Docker labels, etcd) with their state, last sync and the proxies they created. Read-only."
    )]
    async fn list_discovery_status(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let sources = super::discovery::list(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(sources.0)
    }

    /// Read a system log.
    #[tool(
        description = "Read the log rows of `id`: `server` for the server log, or a port or proxy id. Optional `since` and `until` (Unix seconds) and `limit` (100 by default, at most 500). Read-only; a proxy outside the account's proxy list is not found."
    )]
    async fn get_logs(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<LogsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let query = serde_json::json!({
            "since": params.since, "until": params.until, "limit": params.limit,
        });
        let query: r3v3rs3_api::log::LogQuery = serde_json::from_value(query)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let rows = super::logs::get(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(params.id),
            axum::extract::Query(query),
        )
        .await
        .map_err(any_error)?;
        json_result(rows.0)
    }

    /// Read the dashboard summary.
    #[tool(
        description = "Show the dashboard summary: port and proxy counts by state, certificates that expire soon and, with the platform on, the app counts. Read-only."
    )]
    async fn get_dashboard(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let summary = super::dashboard::get(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(summary.0)
    }

    /// Read the HTTP access log.
    #[tool(
        description = "List recent HTTP access log entries, newest first. Optional filters: `resource_id` (a proxy id), `status`, `search` (a text in the host, path or client), `country` (ISO code), `since_id` and `limit`. An account with a proxy list sees only its proxies. Read-only."
    )]
    async fn get_traffic(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<TrafficQueryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let query = params.query()?;
        let entries = super::traffic::get(axum::Extension(caller), axum::extract::Query(query))
            .await
            .map_err(any_error)?;
        json_result(entries.0)
    }

    /// Read one access log entry.
    #[tool(
        description = "Show the access log entry `id` with its request and response headers. Read-only."
    )]
    async fn get_traffic_entry(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<TrafficEntryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let entry =
            super::traffic::get_entry(axum::Extension(caller), axum::extract::Path(params.id))
                .await
                .map_err(any_error)?;
        json_result(entry.0)
    }

    /// Count the access log by country.
    #[tool(
        description = "Count recent requests by client country. Takes the same filters as `get_traffic`. Read-only."
    )]
    async fn get_traffic_geo(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<TrafficQueryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let query = params.query()?;
        let geo = super::traffic::geo(axum::Extension(caller), axum::extract::Query(query))
            .await
            .map_err(any_error)?;
        json_result(geo.0)
    }

    /// Send a captured request again.
    #[tool(
        description = "Send an HTTP request and return the answer. `request` holds `method`, `url`, `headers` (a list of `name`/`value`), optional `body` and `timeout_ms`. Private and loopback addresses are refused unless `allow_internal` is true. Needs a token with the edit scope and an admin account."
    )]
    async fn replay_traffic_request(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::traffic::RequestReplayRequest = json_arg(params.request)?;
        let answer = super::traffic::replay(axum::Extension(caller), axum::Json(request))
            .await
            .map_err(any_error)?;
        json_result(answer.0)
    }

    /// Read the anomaly alert rules.
    #[tool(
        description = "Show the anomaly alert rules: `enabled` and the thresholds `max_p99_latency_ms`, `max_5xx_percent`, `max_request_rate_rps` and `max_event_loop_lag_ms`. Read-only."
    )]
    async fn get_anomaly_rules(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let rules = super::anomaly::get_rules(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(rules.0)
    }

    /// Replace the anomaly alert rules.
    #[tool(
        description = "Replace the anomaly alert rules with `request` (the fields that `get_anomaly_rules` returns; a missing threshold turns that alert off). Needs a token with the edit scope and an account that may edit."
    )]
    async fn update_anomaly_rules(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let rules: r3v3rs3_api::anomaly::AnomalyAlertRules = json_arg(params.request)?;
        let stored = super::anomaly::update_rules(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(rules),
        )
        .await
        .map_err(any_error)?;
        json_result(stored.0)
    }

    /// Dismiss the current anomaly alerts.
    #[tool(
        description = "Dismiss every current anomaly alert. New anomalies raise new alerts. Needs a token with the edit scope and an account that may edit."
    )]
    async fn dismiss_anomalies(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        super::anomaly::dismiss(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(serde_json::json!({ "dismissed": true }))
    }

    /// Grade a set of response header rules.
    #[tool(
        description = "Grade the response header rules `request.rules` (the `header_rules` of a proxy) against the OWASP secure header recommendations: returns `score`, `grade`, one item per header and the matching profile. Read-only."
    )]
    async fn analyze_security_headers(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let body: r3v3rs3_api::security_headers::AnalyzeHeadersRequest = json_arg(params.request)?;
        let audit =
            super::security_headers::analyze_headers(axum::Extension(caller), axum::Json(body))
                .await
                .map_err(any_error)?;
        json_result(audit.0)
    }

    /// Show where a proxy would send a request.
    #[tool(
        description = "Run a request through a proxy definition without sending it. `request` holds `proxy` (an HTTP proxy as in `create_proxy`), `url`, optional `method` and `client_ip`. Returns whether a route matched, the route, the upstream URL or the redirect. Read-only."
    )]
    async fn preview_proxy(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let request: r3v3rs3_api::proxy::ProxyDryRunRequest = json_arg(params.request)?;
        let result = super::proxies::dry_run(axum::Extension(caller), axum::Json(request))
            .await
            .map_err(any_error)?;
        json_result(result.0)
    }

    /// The worker processes that serve the ports.
    #[tool(
        description = "Show the state of the worker processes that serve the ports: the mode (`workers = off` means the master serves them itself), and the pid, start time and restart count of each worker. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_workers(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let workers = super::workers::list(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(workers.0)
    }

    /// Replace the global settings.
    #[tool(
        description = "Replace the global settings of the server. `request` is the whole settings object as `get_config` returns it, with your changes: read it first, change the fields, and send it back. Masked secret values keep the saved secret, so a round trip does not erase keys or passwords. Needs a token with the edit scope and an admin account."
    )]
    async fn update_config(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let config: r3v3rs3_api::app::AppConfig = json_arg(params.request)?;
        let saved = super::config::put(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(config),
        )
        .await
        .map_err(any_error)?;
        json_result(saved.0)
    }

    /// Replace a Git connection.
    #[tool(
        description = "Replace the Git connection `id`. `request` is the body of PUT /api/git/connections/{id}: `name`, `provider` (`gitlab` or `gitea`), `url`, `client_id` and, only when it changes, `client_secret`. The secret is never returned. Needs a token with the edit scope and an admin account."
    )]
    async fn update_git_connection(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdJsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let request: r3v3rs3_api::git_connection::GitConnectionRequest = json_arg(params.request)?;
        let entry = super::git::update_connection(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(entry.0)
    }

    /// Start the OAuth authorization of a Git connection.
    #[tool(
        description = "Start the OAuth authorization of the Git connection `id`: returns the provider URL that an account opens in a browser to grant access. `request` holds `redirect_uri`, the `/oauth/git/callback` URL of this panel. Needs a token with the edit scope and an admin account."
    )]
    async fn authorize_git_connection(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdJsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let request: r3v3rs3_api::git_connection::AuthorizeRequest = json_arg(params.request)?;
        let answer = super::git::authorize_connection(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(answer.0)
    }

    /// Create the manifest form of a private GitHub App.
    #[tool(
        description = "Create the manifest form that registers a private GitHub App for this panel. `request` holds `name`, `redirect_uri` (the `/oauth/git/callback` URL of this panel), optional `organization` and, for GitHub Enterprise, `url`. Open the returned form in a browser to finish the registration on GitHub. Needs a token with the edit scope and an admin account."
    )]
    async fn create_github_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<JsonRequestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::git_connection::GithubAppRequest = json_arg(params.request)?;
        let form = super::git::create_github_app(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(form.0)
    }

    /// Read the installation of a GitHub App connection.
    #[tool(
        description = "Read the installation of the GitHub App connection `id` from GitHub after the app was installed on an account or organization, and store it. Refused for GitLab and Gitea connections. Needs a token with the edit scope and an admin account."
    )]
    async fn install_github_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let entry = super::git::install_github_app(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(entry.0)
    }

    /// Read one project.
    #[tool(
        description = "Show the project `id` with its environments. Read-only; any account can call it."
    )]
    async fn get_project(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let project = super::platform::get_project(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(project.0)
    }

    /// Rename a project or change its description.
    #[tool(
        description = "Replace the `spec` of the project `id`: `name`, `description` and `default_environments`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_project(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateProjectParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let project = super::platform::update_project(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(params.spec),
        )
        .await
        .map_err(any_error)?;
        json_result(project.0)
    }

    /// Read one environment of a project.
    #[tool(
        description = "Show the environment `environment_id` of the project `project_id`. Read-only; any account can call it."
    )]
    async fn get_environment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ProjectEnvironmentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let path = params.ids()?;
        let env = super::platform::get_environment(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(path),
        )
        .await
        .map_err(any_error)?;
        json_result(env.0)
    }

    /// Rename an environment or change its description.
    #[tool(
        description = "Change the environment `environment_id` of the project `project_id`. `request` holds the new `name` and optional `description`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_environment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateEnvironmentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let path = (
            parse_id(&params.project_id)?,
            parse_id(&params.environment_id)?,
        );
        let request: r3v3rs3_api::platform::EnvironmentSpec = json_arg(params.request)?;
        let env = super::platform::update_environment(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(path),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(env.0)
    }

    /// Delete an environment of a project.
    #[tool(
        description = "Delete the environment `environment_id` of the project `project_id`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_environment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ProjectEnvironmentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let path = params.ids()?;
        let env = super::platform::delete_environment(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(path),
        )
        .await
        .map_err(any_error)?;
        json_result(env.0)
    }

    /// Read the variables of an environment.
    #[tool(
        description = "List the variables of the environment `environment_id` of the project `project_id`. Secret values are never returned. Read-only; any account can call it."
    )]
    async fn get_environment_env(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ProjectEnvironmentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let path = params.ids()?;
        let vars = super::platform::get_environment_env(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(path),
        )
        .await
        .map_err(any_error)?;
        json_result(vars.0)
    }

    /// Compare the variables of two environments.
    #[tool(
        description = "Compare the variables of the environments `source_id` and `target_id` of the project `project_id`: keys only in one, and keys whose values differ. Secret values are never returned. Read-only; any account can call it."
    )]
    async fn diff_environments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DiffEnvironmentsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let path = (
            parse_id(&params.project_id)?,
            parse_id(&params.source_id)?,
            parse_id(&params.target_id)?,
        );
        let report = super::platform::diff_environments(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(path),
        )
        .await
        .map_err(any_error)?;
        json_result(report.0)
    }

    /// Read one env group.
    #[tool(
        description = "Show the environment variable group `id` with its variables. Secret values are never returned. Read-only; any account can call it."
    )]
    async fn get_env_group(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let group = super::platform::get_env_group(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(group.0)
    }

    /// Replace an env group.
    #[tool(
        description = "Replace the environment variable group `id` with `group` (`name`, `description`, `variables`). A secret variable sent without a value keeps its stored value. Apps that use the group get the new variables at their next deployment. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_env_group(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateEnvGroupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let group = super::platform::update_env_group(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(params.group),
        )
        .await
        .map_err(any_error)?;
        json_result(group.0)
    }

    /// Read one deployment.
    #[tool(
        description = "Show one deployment (`id` from `list_app_deployments`): its app, state, image, commit, timestamps and error. Read-only; any account can call it."
    )]
    async fn get_deployment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let entry = super::platform::get_deployment(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(entry.0)
    }

    /// Approve a deployment that waits for approval.
    #[tool(
        description = "Approve the deployment `id` that waits for approval (apps with `require_approval` or outside their `deploy_window`); it starts at once. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn approve_deployment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let entry = super::platform::approve_deployment(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(entry.0)
    }

    /// Reject a deployment that waits for approval.
    #[tool(
        description = "Reject the deployment `id` that waits for approval; it never starts. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn reject_deployment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let entry = super::platform::reject_deployment(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(entry.0)
    }

    /// Read the CPU and memory history of an app.
    #[tool(
        description = "Show the CPU, memory and network history of the app `id` over the last `hours` (1 to 24, default 6). Read-only; any account can call it."
    )]
    async fn get_app_metrics(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AppMetricsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let query = r3v3rs3_api::platform::MetricsQuery {
            hours: params.hours,
        };
        let points = super::platform::get_app_metrics(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::extract::Query(query),
        )
        .await
        .map_err(any_error)?;
        json_result(points.0)
    }

    /// Read the state of each service of a Compose app.
    #[tool(
        description = "Show the state, health and ports of each service of the Compose app `id`. Read-only; any account can call it."
    )]
    async fn get_app_services(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let services = super::platform::get_services(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(services.0)
    }

    /// Read the service graph of a Compose app.
    #[tool(
        description = "Show the services of the Compose app `id` with their dependencies, networks and volumes. Read-only; any account can call it."
    )]
    async fn get_app_topology(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let topology = super::platform::get_topology(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(topology.0)
    }

    /// Move an App Store app to the newer catalog entry.
    #[tool(
        description = "Update the app `id`, installed from the App Store, to the image of its newer catalog entry (see `catalog_update` in `list_apps`), and return the app. Deploy it afterwards with `deploy_app`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn upgrade_app_catalog(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let app = super::platform::upgrade_app_catalog(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(app.0)
    }

    /// Set the Git token that clones a private repository.
    #[tool(
        description = "Set the access `token` that clones the private repository of the Git or Compose app `id` without a Git connection. The token is sealed and never returned. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_app_git_token(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<GitTokenParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let request = r3v3rs3_api::platform::GitTokenRequest {
            token: params.token,
        };
        let app = super::platform::put_git_token(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(app.0)
    }

    /// Remove the Git token of an app.
    #[tool(
        description = "Remove the stored Git access token of the app `id`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_app_git_token(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let app = super::platform::delete_git_token(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(app.0)
    }

    /// Create a new push webhook secret for an app.
    #[tool(
        description = "Create a new push webhook secret for the app `id` and return it with the webhook URL. The secret is shown only in this answer; the old one stops working. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_app_webhook_secret(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let secret = super::platform::new_webhook_secret(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(secret.0)
    }

    /// Remove the push webhook secret of an app.
    #[tool(
        description = "Remove the push webhook secret of the app `id`; pushes no longer deploy it. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_app_webhook_secret(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let app = super::platform::delete_webhook_secret(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(app.0)
    }

    /// Install the push webhook in the Git provider.
    #[tool(
        description = "Install the push webhook of the app `id` in its repository through the app's Git connection, so a push deploys it. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn install_app_hook(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let app = super::platform::install_hook(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(app.0)
    }

    /// Take an immediate database backup snapshot.
    #[tool(
        description = "Take a backup of a managed database (`id`) now: a compressed dump made with the dump tool of its engine, copied to the configured S3, SFTP or rsync targets. Afterwards the oldest backups beyond the `keep` count are deleted. Returns the new backup entry. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_database_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let entry = super::backups::create(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(entry.0)
    }

    /// Delete a database backup snapshot.
    #[tool(
        description = "Delete one backup (`backup_id` from `list_database_backups`) of a managed database (`db_id`). This cannot be undone. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_database_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DeleteBackupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.db_id)?;
        let axum::Json(()) = super::backups::delete(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path((id, params.backup_id.clone())),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "deleted": params.backup_id }))
    }

    /// Get database backup storage configuration.
    #[tool(
        description = "Show the backup settings of the managed databases: the daily backup time (`schedule`, `HH:MM`), how many backups to keep, the S3, SFTP and rsync copy targets, restore testing and the heartbeat URL. Secret keys and passwords come back empty. Takes no arguments. Read-only; any account without a proxy list."
    )]
    async fn get_backup_config(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let mut config = platform.backup_config().await.map_err(any_error)?;
        if let Some(s3) = config.s3.as_mut() {
            s3.secret_key.clear();
        }
        if let Some(sftp) = config.sftp.as_mut() {
            sftp.password.clear();
            sftp.key.clear();
        }
        if let Some(rsync) = config.rsync.as_mut() {
            rsync.password.clear();
        }
        json_result(config)
    }

    /// Update database backup storage settings.
    #[tool(
        description = "Replace the whole backup configuration with `config` (start from `get_backup_config`; omitted fields fall back to their defaults). An empty secret key or password keeps the stored one. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_backup_config(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateBackupConfigParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let axum::Json(()) = super::backups::put_config(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(params.config),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "updated": true }))
    }

    /// Test decompression and format integrity of a database backup snapshot.
    #[tool(
        description = "Check that one backup (`db_id`, `backup_id`) is usable: verify its SHA-256 checksum, decompress it and validate the dump format, without touching the running database. Returns the verification report and records the result on the backup; a failure also sends the `backup.verification_failed` notification. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn test_database_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<TestBackupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.db_id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let report = platform
            .verify_backup(id, &params.backup_id)
            .await
            .map_err(any_error)?;
        let app = platform.app(id).await.map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::BackupDatabase)
            .id(id)
            .summary(format!("{}: test backup {}", app.name, params.backup_id));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(report)
    }

    /// Get the point-in-time recovery (PITR) window and available WAL segments for a database.
    #[tool(
        description = "Show the point-in-time recovery window of a managed database (`id`): the earliest and latest restorable time (Unix milliseconds), the number of base backups and archived log files (PostgreSQL WAL, MySQL and MariaDB binary log, MongoDB oplog, Redis AOF) over every timeline, and whether the running server archives its log; a database that ran before archiving starts it on its next deployment. Pick `target_time` for `restore_database_pitr` inside this window. Read-only; any account without a proxy list."
    )]
    async fn get_database_pitr_range(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let range = platform.pitr_range(id).await.map_err(any_error)?;
        json_result(range)
    }

    /// Restore a database to a specific point in time (PITR).
    #[tool(
        description = "Restore a managed database (`id`) to the state at `target_time` (Unix milliseconds, inside the window of `get_database_pitr_range`) from a base backup plus the archived change log, replayed up to the second before the target. Without `new_name` the running database is overwritten in place, and the archive before the recovery stays reachable for later targets; with `new_name` (and optional `host_port`) a new database is created from that point and the original stays untouched. Returns the restored app, the base backup used and the number of segments replayed. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn restore_database_pitr(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<RestoreDatabasePitrParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let new_name = params
            .new_name
            .map(|n| n.parse::<r3v3rs3_api::container::AppName>())
            .transpose()
            .map_err(|e| ErrorData::invalid_params(format!("invalid new_name: {e}"), None))?;
        let request = r3v3rs3_api::platform::PitrRestoreRequest {
            target_time: params.target_time,
            new_name,
            host_port: params.host_port,
        };
        let resp = platform
            .restore_pitr(id, request)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::PitrRestoreDatabase)
            .id(id)
            .summary(format!(
                "{}: PITR target {} (branched: {})",
                resp.app.name, resp.target_time, resp.branched
            ));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(resp)
    }

    /// Archive the current WAL or binary log segment from the running database container.
    #[tool(
        description = "Capture the current change log of a running managed database (`id`) and archive it now (PostgreSQL WAL, MySQL and MariaDB binary log, MongoDB oplog, Redis AOF), so the point-in-time recovery window reaches the present. Returns the archived segment. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn archive_database_wal(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let segment = platform.archive_wal(id).await.map_err(any_error)?;
        let app = platform.app(id).await.map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::ArchiveDatabaseWal)
            .id(id)
            .summary(format!("{}: archived WAL {}", app.name, segment.filename));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(segment)
    }

    /// List archived WAL and binary log segments for a database.
    #[tool(
        description = "List the archived change log segments (WAL, binary log, oplog or AOF) of a managed database (`id`) with file name, size and the time range each one covers. Read-only; any account without a proxy list."
    )]
    async fn list_database_wal_segments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let segments = platform.list_wal_segments(id).await.map_err(any_error)?;
        json_result(segments)
    }

    // ==========================================
    // Group 9: Agent Targets & Nodes
    // ==========================================

    /// List all Docker host targets and agent nodes.
    #[tool(
        description = "List the deployment targets: the `local` target (the Docker engine of this server) and every agent node, with id, name, kind, region, whether the agent enrolled and is online, last contact, agent version, drained flag and link quality (latency, jitter, packet loss). Use a target id as `target` of an app. Takes no arguments. Read-only; any account without a proxy list."
    )]
    async fn list_targets(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let targets = platform.targets().await.map_err(any_error)?;
        json_result(targets)
    }

    /// Add an agent target and generate enrollment token.
    #[tool(
        description = "Add an agent target (another server that runs apps for this one). `request` holds `name` and an optional `region`. Returns the target, a one-time enrollment token and the agent host and port to dial; run the agent on the remote server with them (`r3-server agent`). The token appears only in this answer. Needs a token with the edit scope and an admin account."
    )]
    async fn create_target(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddTargetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let token = super::platform::add_target(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(params.request),
        )
        .await
        .map_err(any_error)?;
        json_result(token.0)
    }

    /// Delete an agent target node.
    #[tool(
        description = "Delete an agent target by its `id` and close the connection of its agent. A target that still has apps is refused; move or delete its apps first. Needs a token with the edit scope and an admin account."
    )]
    async fn delete_target(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let axum::Json(()) = super::platform::delete_target(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }

    /// Set or clear the geographical region of a target node.
    #[tool(
        description = "Set the region label of a target (`id`, `region`, for example `eu`, `na`, `tr` or `apac`; null clears it). GeoIP steering sends clients to targets of their region. Returns the updated target. Needs a token with the edit scope and an admin account."
    )]
    async fn update_target_region(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<SetTargetRegionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let updated = super::platform::set_target_region(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
            axum::Json(r3v3rs3_api::platform::SetTargetRegionRequest {
                region: params.region,
            }),
        )
        .await
        .map_err(any_error)?;
        json_result(updated.0)
    }

    /// Drain a target node for maintenance.
    #[tool(
        description = "Drain a target (`id`) for maintenance: it leaves the load-balancing pools, so apps that run on several targets stop sending it new traffic. Its containers keep running. Undo it with `undrain_target`. Returns the updated target. Needs a token with the edit scope and an admin account."
    )]
    async fn drain_target(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let updated = super::platform::drain_target(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(updated.0)
    }

    /// Undrain a target node, restoring it to the active traffic pool.
    #[tool(
        description = "Return a drained target (`id`) to the load-balancing pools, so it gets traffic again. Returns the updated target. Needs a token with the edit scope and an admin account."
    )]
    async fn undrain_target(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let updated = super::platform::undrain_target(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(updated.0)
    }

    /// Generate a new enrollment token for an agent target node.
    #[tool(
        description = "Replace the enrollment token of an agent target (`id`), for example when the old token leaked or a server is reinstalled. The enrolled agent keeps working until an agent enrolls with the new token. Returns the new token (shown only here) with the agent host and port. Needs a token with the edit scope and an admin account."
    )]
    async fn create_target_token(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let created = platform.new_target_token(id).await.map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::NewTargetToken)
            .id(id)
            .summary(created.target.name.clone());
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(created)
    }

    /// Upgrade target agent binary to match the server version.
    #[tool(
        description = "Push the binary of this server to the agent of one target (`id`) so the agent runs the same version as the server. The agent must be online. Returns the updated target. Needs a token with the edit scope and an admin account."
    )]
    async fn upgrade_target(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let updated = platform.upgrade_target(id).await.map_err(any_error)?;
        let version = crate::release::version().map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::UpgradeTarget)
            .id(id)
            .summary(format!("{} -> {version}", updated.name));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(updated)
    }

    /// Upgrade all agent target binaries to match the server version.
    #[tool(
        description = "Push the binary of this server to every connected agent, so all agents run the server version. A failure on one agent is logged and does not stop the others. Returns the targets that were upgraded. Takes no arguments. Needs a token with the edit scope and an admin account."
    )]
    async fn upgrade_all_targets(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let upgraded = super::platform::upgrade_all_targets(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(upgraded.0)
    }

    // ==========================================
    // Group 10: Projects & Environment Variable Groups
    // ==========================================

    /// List all platform projects.
    #[tool(
        description = "List every project of the platform with its id, name, description, number of environments and number of apps; `list_project_environments` lists the environments of one project. A project groups apps; each of its environments (for example staging and production) holds its own variables and its own tenant on a shared database. Takes no arguments. Read-only; any account without a proxy list."
    )]
    async fn list_projects(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let projects = platform.list_projects().await.map_err(any_error)?;
        json_result(projects)
    }

    /// Create a new project.
    #[tool(
        description = "Create a project from `spec`: `name`, an optional `description` and `default_environments` (names of environments to create with it; without them a `production` environment is created). Returns the new project with its id. Assign apps to it through `project_id` and `environment_id` in their spec. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_project(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateProjectParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let project = super::platform::create_project(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(params.spec),
        )
        .await
        .map_err(any_error)?;
        json_result(project.0)
    }

    /// Delete a project by id.
    #[tool(
        description = "Delete a project by its `id` with its environments and their variables. A project that apps still belong to is refused; move or delete those apps first. Returns the deleted project. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_project(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let project = super::platform::delete_project(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(project.0)
    }

    /// List all shared environment variable groups.
    #[tool(
        description = "List every shared environment variable group with its id, name, description and number of variables (the variables themselves are not listed). An app that names a group in its `env_groups` gets all of its variables at deployment. Takes no arguments. Read-only; any account without a proxy list."
    )]
    async fn list_env_groups(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let groups = platform.list_env_groups().await.map_err(any_error)?;
        json_result(groups)
    }

    /// Create a shared environment variable group.
    #[tool(
        description = "Create a shared environment variable group from `group`: `name`, an optional `description` and `variables` ([{key, value, secret}]). Returns the new group with its id; add the id to the `env_groups` of an app to use it. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_env_group(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateEnvGroupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let created = super::platform::create_env_group(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(params.group),
        )
        .await
        .map_err(any_error)?;
        json_result(created.0)
    }

    /// Delete an environment variable group by id.
    #[tool(
        description = "Delete a shared environment variable group by its `id`. A group that an app still lists in its `env_groups` is refused. Returns the deleted group. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn delete_env_group(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let deleted = super::platform::delete_env_group(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(id),
        )
        .await
        .map_err(any_error)?;
        json_result(deleted.0)
    }

    /// List all environments belonging to a project.
    #[tool(
        description = "List the environments of a project (`id`) with their id, name, description, number of variables and number of apps (the variables themselves are not listed). Use an environment id for `update_environment_env`, `sync_environment_env` or the `environment_id` of an app. Read-only; any account without a proxy list."
    )]
    async fn list_project_environments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let project = platform.project(id).await.map_err(any_error)?;
        json_result(project.environments)
    }

    /// Create a new environment within a project. Needs the edit scope.
    #[tool(
        description = "Create an environment in a project (`project_id`, `name` such as `staging` or `preview`, optional `description`). It starts without variables; set them with `update_environment_env` or copy them with `sync_environment_env`. Returns the new environment with its id. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn create_environment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateProjectEnvironmentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let project_id: ShortId = parse_id(&params.project_id)?;
        let spec = r3v3rs3_api::platform::EnvironmentSpec {
            name: params.name,
            description: params.description,
        };
        spec.validate().map_err(any_error)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let env = platform
            .create_environment(project_id, spec, crate::clock::unix_ms())
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::AddEnvironment)
            .id(env.id)
            .summary(env.name.clone());
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(env)
    }

    /// Update environment variables for a project environment. Needs the edit scope.
    #[tool(
        description = "Replace the whole variable list of a project environment (`environment_id`) with `variables` ([{key, value, secret}]); a variable missing from the list is deleted. The apps of the environment get the new values at their next deployment. Returns the stored variables. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_environment_env(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateProjectEnvParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let (env_id, spec) = parse_project_env_params(params)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let stored = platform
            .store
            .environment(env_id)
            .await
            .map_err(any_error)?
            .ok_or_else(|| any_error(crate::platform::not_found(env_id)))?;
        let env = platform
            .set_environment_env(stored.project_id, env_id, spec)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::UpdateEnvironmentEnv)
            .id(env_id)
            .summary(env.name.clone());
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(env.variables)
    }

    /// Synchronize environment variables from source to target environment. Needs the edit scope.
    #[tool(
        description = "Copy variables from one environment of a project to another (`project_id`, `source_env_id`, `target_env_id`). `mode`: `sync_missing` (default) adds only the keys the target lacks; `clone_all` replaces all target variables with the source ones; `selected_keys` copies only the keys in `keys`. Returns how many variables were synced and which keys were added or updated. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn sync_environment_env(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<SyncProjectEnvParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let project_id: ShortId = parse_id(&params.project_id)?;
        let request = sync_request(&params)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let report = platform
            .sync_environment_env(project_id, request)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::SyncEnvironmentEnv)
            .id(report.target_env_id)
            .summary(format!(
                "{} -> {} ({} synced)",
                report.source_env_id, report.target_env_id, report.synced_count
            ));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(report)
    }

    // ==========================================
    // Group 11: Disaster Recovery
    // ==========================================

    /// List all full disaster recovery backup snapshots.
    #[tool(
        description = "List the disaster recovery bundles stored on this server with their id, creation time, size, whether they are encrypted and the number of files they hold. Takes no arguments. Read-only; admin accounts only."
    )]
    async fn list_disaster_recovery_backups(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let backups = super::disaster_recovery::list(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
        )
        .await
        .map_err(any_error)?;
        json_result(backups.0)
    }

    /// Create a full system disaster recovery backup snapshot.
    #[tool(
        description = "Pack the whole server state into one AES-256-GCM encrypted disaster recovery bundle: settings, ports, proxies, access lists, certificates, accounts, the platform database and the platform secrets key. `passphrase` encrypts it; keep it safe, because the bundle cannot be restored without it. `upload_s3` (default true) also copies the bundle to the S3 bucket of the backup settings. Returns the new bundle entry. Needs a token with the edit scope and an admin account."
    )]
    async fn create_disaster_recovery_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateDrBackupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request = r3v3rs3_api::platform::CreateDisasterRecoveryRequest {
            passphrase: Some(params.passphrase),
            upload_s3: params.upload_s3.unwrap_or(true),
        };
        let backup = super::disaster_recovery::create(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(backup.0)
    }

    /// Restore a full system disaster recovery backup snapshot by ID.
    #[tool(
        description = "Overwrite the current server state with a disaster recovery bundle (`backup_id` from `list_disaster_recovery_backups`), decrypted with its `passphrase`. Every setting, proxy, account and platform record made after the bundle is lost. The server must be restarted afterwards to load the restored state. Returns the number of restored files. Needs a token with the edit scope and an admin account."
    )]
    async fn restore_disaster_recovery_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<RestoreDrBackupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request = r3v3rs3_api::platform::RestoreDisasterRecoveryRequest {
            passphrase: Some(params.passphrase),
        };
        let restored = super::disaster_recovery::restore(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(params.backup_id),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(serde_json::json!({
            "status": "restored",
            "files_restored": restored.0,
            "message": "Disaster recovery snapshot restored successfully. Please restart r3v3rs3 to apply changes."
        }))
    }

    /// Delete a full system disaster recovery backup snapshot by ID.
    #[tool(
        description = "Delete one disaster recovery bundle (`backup_id`) from this server. A copy in S3 is not deleted. Needs a token with the edit scope and an admin account."
    )]
    async fn delete_disaster_recovery_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DeleteDrBackupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let _ = super::disaster_recovery::delete(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::extract::Path(params.backup_id),
        )
        .await
        .map_err(any_error)?;
        json_result(serde_json::json!({
            "status": "deleted"
        }))
    }

    /// Delete the rotated container log files across the platform.
    #[tool(
        description = "Delete the rotated container log files of every app on the platform to free disk space at once; with `threshold_mb` only the containers whose log files are larger than that many megabytes lose them. The active log files stay. The deleted lines are lost. Returns the number of deleted files and the bytes freed. Needs a token with the edit scope and an admin account."
    )]
    async fn vacuum_platform_logs(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<VacuumLogsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let res = platform
            .vacuum_all_logs(params.threshold_mb)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::VacuumLogs).summary(format!(
            "Platform vacuumed container logs, freed {} bytes",
            res.freed_bytes
        ));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(res)
    }

    /// Execute automated Docker disk hygiene prune immediately.
    #[tool(
        description = "Run the Docker disk cleanup now on the Docker host of the r3v3rs3 server and on every agent target, with the switches of the prune policy (`get_prune_policy`): unused images, stopped containers, build cache, unused volumes and large container logs. On an agent host only the stopped platform containers, the dangling image layers, the build cache and the rotated logs of the platform apps go. The newest builds of each app stay for rollback. An offline agent fails the call with its name after the server host is cleaned. Returns the bytes reclaimed. Takes no arguments. Needs a token with the edit scope and an admin account."
    )]
    async fn run_prune(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let res = platform.run_prune().await.map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::UpdatePlatformSettings).summary(format!(
            "Ran disk prune, reclaimed {} bytes",
            res.space_reclaimed_bytes
        ));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(res)
    }

    /// Read the Docker disk hygiene and garbage collector prune policy.
    #[tool(
        description = "Show the Docker disk cleanup policy: whether the scheduler runs and how often, the disk usage percentage that triggers an emergency cleanup, which kinds of data it prunes (images, volumes, build cache, containers, logs), the log size threshold, the build cache to keep, the number of builds kept per app and the container log rotation limits. Takes no arguments. Read-only; any account without a proxy list."
    )]
    async fn get_prune_policy(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let policy = platform.prune_policy().await.map_err(any_error)?;
        json_result(policy)
    }

    /// Update the Docker disk hygiene and garbage collector prune policy.
    #[tool(
        description = "Change the Docker disk cleanup policy. Pass only the fields to change; every omitted field keeps its value. Pruning volumes (`prune_volumes`) deletes the data of unused volumes for good. Returns the new policy. Needs a token with the edit scope and an admin account."
    )]
    async fn update_prune_policy(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdatePrunePolicyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let mut policy = platform.prune_policy().await.map_err(any_error)?;
        apply_prune_policy_overrides(&mut policy, &params);
        platform
            .update_prune_policy(policy.clone())
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::UpdatePlatformSettings).summary(format!(
            "Updated prune policy via MCP (enabled: {})",
            policy.enabled
        ));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(policy)
    }

    /// Read the host node resource threshold alert rules.
    #[tool(
        description = "Show the host resource alert rules: whether they are on, and the thresholds for low free disk space (percent), bandwidth spikes (bytes per second), open file descriptors (percent of the limit) and TCP TIME_WAIT sockets. A crossed threshold sends a `system.*` notification. Takes no arguments. Read-only; any account without a proxy list."
    )]
    async fn get_node_alerts(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let rules = platform.node_alert_rules().await.map_err(any_error)?;
        json_result(rules)
    }

    /// Update the host node resource threshold alert rules.
    #[tool(
        description = "Replace all host resource alert rules. `enabled` is required; each threshold you omit is cleared, which turns that alert off, so pass every threshold you want to keep (read them with `get_node_alerts`). Returns the stored rules. Needs a token with the edit scope and an admin account."
    )]
    async fn update_node_alerts(
        &self,
        Parameters(params): Parameters<UpdateNodeAlertsParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let rules = r3v3rs3_api::platform::NodeAlertRules {
            enabled: params.enabled,
            min_free_disk_percent: params.min_free_disk_percent,
            bandwidth_spike_bytes_per_sec: params.bandwidth_spike_bytes_per_sec,
            max_fd_ratio_percent: params.max_fd_ratio_percent,
            max_time_wait_sockets: params.max_time_wait_sockets,
        };
        rules.validate().map_err(any_error)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        platform
            .update_node_alert_rules(rules)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::UpdateNodeAlertRules).summary(format!(
            "Updated node alert rules via MCP (enabled: {})",
            rules.enabled
        ));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(rules)
    }

    /// List active long-lived connections and streams.
    #[tool(
        description = "List the open long-lived connections with their connection id, protocol (`web_socket`, `tcp`, `sse` or `http_upgrade` for other HTTP upgrades and HTTP/2 CONNECT), client address, proxy id and name, upstream address, start time and uptime, bytes sent and received and current rate in bytes per second. `proxy_id` limits the list to one proxy. An account with a proxy list sees only the connections of its proxies. Read-only; any account can call it."
    )]
    async fn list_traffic_connections(
        &self,
        Parameters(params): Parameters<ListActiveConnectionsParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let proxy_id = params
            .proxy_id
            .as_deref()
            .map(|s| s.parse::<ShortId>())
            .transpose()
            .map_err(|e| ErrorData::invalid_params(format!("invalid proxy_id: {e}"), None))?;
        let conns = crate::proxy::connection_tracker::list(proxy_id.as_ref());
        let filtered: Vec<_> = conns
            .into_iter()
            .filter(|c| match c.proxy_id {
                Some(id) => caller.can_see(id),
                None => caller.proxies.is_none(),
            })
            .collect();
        json_result(filtered)
    }

    /// Terminate an active connection or stream by its unique ID.
    #[tool(
        description = "Close one open long-lived connection now by its connection `id` from `list_traffic_connections`. The client may reconnect. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn terminate_traffic_connection(
        &self,
        Parameters(params): Parameters<IdParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        caller.authorize(Permission::Edit).map_err(tool_error)?;
        let conn_id = params
            .id
            .parse::<ShortId>()
            .map_err(|e| ErrorData::invalid_params(format!("invalid connection id: {e}"), None))?;
        crate::proxy::connection_tracker::terminate(conn_id).map_err(|e| {
            ErrorData::internal_error(format!("failed to terminate connection: {e}"), None)
        })?;
        let record = AuditRecord::new(AuditAction::TerminateConnection)
            .summary(format!("Terminated active connection {} via MCP", conn_id));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(serde_json::json!({
            "status": "terminated",
            "id": conn_id.to_string(),
        }))
    }

    /// Get the Certificate Transparency (CT) monitor configuration.
    #[tool(
        description = "Show the settings of the Certificate Transparency monitor, which searches public CT logs for certificates issued for your domains: whether it runs, the check interval, the monitored domains, whether domains are discovered from your certificates and ACME requests, the trusted CA issuers, whether an unknown issuer raises an alert, and the CT search API URL. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_ct_config(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        caller.authorize(Permission::Read).map_err(tool_error)?;
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = super::ct::get_or_init_state(&config_dir);
        json_result(monitor.get_config())
    }

    /// Update the Certificate Transparency (CT) monitor configuration.
    #[tool(
        description = "Change the settings of the Certificate Transparency monitor. Pass only the fields to change; every omitted field keeps its value, and a list you pass replaces the whole list. Returns the stored settings. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn update_ct_config(
        &self,
        Parameters(params): Parameters<UpdateCtConfigParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        caller.authorize(Permission::Edit).map_err(tool_error)?;
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = super::ct::get_or_init_state(&config_dir);
        let mut config = monitor.get_config();
        apply_ct_config_overrides(&mut config, params);

        monitor
            .save_config(config.clone())
            .map_err(|e| ErrorData::internal_error(format!("failed to save config: {e}"), None))?;

        let record = AuditRecord::new(AuditAction::UpdateCtMonitorConfig)
            .summary("Updated Certificate Transparency monitor configuration via MCP");
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(config)
    }

    /// Get the Certificate Transparency (CT) monitor runtime status.
    #[tool(
        description = "Show the state of the Certificate Transparency monitor: the time of the last check, the next scheduled check, the number of open rogue certificate alerts and the number of log entries it tracks. Takes no arguments. Read-only; any account can call it."
    )]
    async fn get_ct_status(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        caller.authorize(Permission::Read).map_err(tool_error)?;
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = super::ct::get_or_init_state(&config_dir);
        json_result(monitor.get_status())
    }

    /// List Certificate Transparency log entries and rogue certificate alerts.
    #[tool(
        description = "List the certificates that the CT monitor found in public logs for your domains, with entry id, domain, issuer, common name, SAN, serial number, log time, validity, whether it is rogue and why, and its alert state. A certificate from an issuer outside the trusted list is flagged as rogue. `domain` filters by domain, `rogue_only` returns only flagged entries, `limit` caps the count (default 50). Read-only; any account can call it."
    )]
    async fn list_ct_entries(
        &self,
        Parameters(params): Parameters<ListCtEntriesParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        caller.authorize(Permission::Read).map_err(tool_error)?;
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = super::ct::get_or_init_state(&config_dir);
        let query = r3v3rs3_api::ct::CtEntriesQuery {
            domain: params.domain,
            rogue_only: params.rogue_only,
            limit: params.limit,
        };
        json_result(monitor.list_entries(query))
    }

    /// Trigger an immediate on-demand check of Certificate Transparency logs.
    #[tool(
        description = "Query the public Certificate Transparency logs now, for `domain` or, without it, for every monitored and discovered domain, and wait for the result. Returns the number of new rogue certificates found. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn check_ct_now(
        &self,
        Parameters(params): Parameters<CheckCtParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        caller.authorize(Permission::Edit).map_err(tool_error)?;
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = super::ct::get_or_init_state(&config_dir);
        let count = monitor
            .run_check(&self.state, params.domain.as_deref())
            .await
            .map_err(|e| ErrorData::internal_error(format!("CT log check failed: {e}"), None))?;

        let record = AuditRecord::new(AuditAction::CheckCtLogs).summary(format!(
            "Triggered Certificate Transparency check via MCP, {count} new rogue certs found"
        ));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(serde_json::json!({
            "status": "completed",
            "new_rogue_certificates_count": count,
        }))
    }

    /// Dismiss or resolve a Certificate Transparency rogue certificate alert.
    #[tool(
        description = "Set the alert state of one CT log entry (`id` from `list_ct_entries`): `dismissed` (default, a known and expected certificate), `investigating`, or `confirmed_rogue`. Needs a token with the edit scope and an admin account or an editor without a proxy list."
    )]
    async fn dismiss_ct_alert(
        &self,
        Parameters(params): Parameters<DismissCtAlertParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        caller.authorize(Permission::Edit).map_err(tool_error)?;
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = super::ct::get_or_init_state(&config_dir);
        let status = Some(parse_optional_choice(
            "status",
            params.status.as_deref(),
            r3v3rs3_api::ct::CtAlertStatus::Dismissed,
        )?);
        monitor.dismiss_alert(&params.id, status).map_err(|e| {
            ErrorData::invalid_params(format!("failed to dismiss alert: {e}"), None)
        })?;

        let record = AuditRecord::new(AuditAction::DismissCtAlert).summary(format!(
            "Dismissed Certificate Transparency alert {} via MCP",
            params.id
        ));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(serde_json::json!({
            "status": "dismissed",
            "id": params.id,
        }))
    }

    // ==========================================
    // Group 12: Outbound Webhooks
    // ==========================================

    /// List all configured outbound webhooks.
    #[tool(
        description = "List every outbound notification webhook with its id, name, delivery URL, whether a signing secret is set (never the secret), enabled flag and subscribed events. r3v3rs3 POSTs a JSON payload to the URL when one of the events happens. Takes no arguments. Read-only; admin accounts only."
    )]
    async fn list_outbound_webhooks(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let webhooks = platform.list_outbound_webhooks().await.map_err(any_error)?;
        json_result(webhooks)
    }

    /// Get details of an outbound webhook by its id.
    #[tool(
        description = "Show one outbound notification webhook by its `id`: name, URL, whether a secret is set, enabled flag, events and times. Read-only; admin accounts only."
    )]
    async fn get_outbound_webhook(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let entry = platform.get_outbound_webhook(id).await.map_err(any_error)?;
        json_result(entry)
    }

    /// Create a new outbound notification webhook.
    #[tool(
        description = "Create an outbound notification webhook: `name` (at most 100 characters), the delivery `url` (http or https; private, loopback and link-local addresses are refused), the `events` to send (at least one), `enabled` (default true) and an optional `secret` (at most 256 characters; each delivery then carries an `X-R3v3rs3-Signature: sha256=<hex HMAC-SHA256 of the body>` header). Event names (`_` may replace `.`): deploy.success, deploy.failure, deploy.pending_approval, app.crashed, app.cpu_throttled, backup.success, backup.failure, backup.upload_failed, backup.restored, backup.verification_failed, cert.expiring, cert.renewed, cert.renewal_failed, cert.rogue_detected, cert.ct_check_failed, security.ip_banned, security.threat_detected, target.offline, system.disk_pressure, system.disk_low, system.bandwidth_spike, system.high_latency, system.high_error_rate, system.traffic_spike, system.event_loop_lag, system.fd_exhaustion, system.socket_exhaustion, system.oom_kill, system.geoip_updated, system.geoip_update_failed. Returns the new webhook. Needs a token with the edit scope and an admin account."
    )]
    async fn create_outbound_webhook(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddWebhookParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let events = parse_webhook_events(&params.events)?;
        let request = r3v3rs3_api::webhook::CreateWebhookRequest {
            name: params.name,
            url: params.url,
            secret: params.secret,
            enabled: params.enabled.unwrap_or(true),
            events,
        };
        request.validate().map_err(any_error)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let entry = platform
            .create_outbound_webhook(request)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::AddOutboundWebhook)
            .id(entry.id)
            .summary(format!("{} ({})", entry.name, entry.url));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(entry)
    }

    /// Update an existing outbound notification webhook.
    #[tool(
        description = "Change an outbound notification webhook by its `id`. Pass only the fields to change: `name`, `url`, `secret` (an empty string removes the secret), `enabled`, or `events` (replaces the whole event list; names as in `create_outbound_webhook`). Returns the updated webhook. Needs a token with the edit scope and an admin account."
    )]
    async fn update_outbound_webhook(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateWebhookParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let events = match params.events {
            Some(evts) => Some(parse_webhook_events(&evts)?),
            None => None,
        };
        let request = r3v3rs3_api::webhook::UpdateWebhookRequest {
            name: params.name,
            url: params.url,
            secret: params.secret,
            enabled: params.enabled,
            events,
        };
        request.validate().map_err(any_error)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let entry = platform
            .update_outbound_webhook(id, request)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::UpdateOutboundWebhook)
            .id(entry.id)
            .summary(format!("{} ({})", entry.name, entry.url));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(entry)
    }

    /// Delete an outbound notification webhook by its id.
    #[tool(
        description = "Delete an outbound notification webhook by its `id`; it receives no more events. Needs a token with the edit scope and an admin account."
    )]
    async fn delete_outbound_webhook(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let entry = platform.get_outbound_webhook(id).await.ok();
        platform
            .delete_outbound_webhook(id)
            .await
            .map_err(any_error)?;
        let summary = entry
            .map(|e| format!("{} ({})", e.name, e.url))
            .unwrap_or_else(|| id.to_string());
        let record = AuditRecord::new(AuditAction::DeleteOutboundWebhook)
            .id(id)
            .summary(summary);
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(json!({ "deleted": true }))
    }

    /// Send a test notification payload to an outbound webhook.
    #[tool(
        description = "Send one test delivery and report whether the receiver accepted it, with its HTTP status code, a message and the elapsed time. Give `webhook_id` to test a saved webhook, or `url` (and optional `secret`) to test an address before saving it. `event` sets the sample event (default `test.ping`). Needs a token with the edit scope and an admin account."
    )]
    async fn test_outbound_webhook(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<TestWebhookParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let webhook_id = match params.webhook_id {
            Some(ref id) => Some(parse_id(id)?),
            None => None,
        };
        let event = params
            .event
            .as_deref()
            .map(parse_webhook_event)
            .transpose()?;
        let request = r3v3rs3_api::webhook::TestWebhookRequest {
            webhook_id,
            url: params.url,
            secret: params.secret,
            event,
        };
        let target_summary = request
            .webhook_id
            .map(|id| id.to_string())
            .or_else(|| request.url.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let res = platform
            .test_outbound_webhook(request)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::TestOutboundWebhook)
            .summary(format!("{target_summary} -> success={}", res.success));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(res)
    }

    // ==========================================
    // Group 13: Container Registries
    // ==========================================

    /// List all configured container registries for image pulls.
    #[tool(
        description = "List the stored container registry logins with their id, registry server, user name and times; passwords are never returned. The platform uses a matching login when it pulls a private image. Takes no arguments. Read-only; any account without a proxy list."
    )]
    async fn list_registries(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Read)
            .await
            .map_err(any_error)?;
        let registries = platform.registries().await.map_err(any_error)?;
        json_result(registries)
    }

    /// Add container registry credentials for private image pulls.
    #[tool(
        description = "Store a login for a private container registry: `server` (for example `ghcr.io` or `registry.gitlab.com`; one login per server), `username` and `password` (required: a password or an access token with pull rights). Deployments of images from that server use it. Returns the new entry without the password. Needs a token with the edit scope and an admin account."
    )]
    async fn create_registry(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddRegistryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request = r3v3rs3_api::platform::RegistryRequest {
            server: params.server,
            username: params.username,
            password: Some(params.password),
        };
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let entry = platform.add_registry(request).await.map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::AddRegistry)
            .id(entry.id)
            .summary(entry.server.clone());
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(entry)
    }

    /// Update container registry credentials.
    #[tool(
        description = "Change a stored registry login by its `id`: `server` and `username` are required; an omitted or empty `password` keeps the stored one. Returns the updated entry without the password. Needs a token with the edit scope and an admin account."
    )]
    async fn update_registry(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateRegistryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let request = r3v3rs3_api::platform::RegistryRequest {
            server: params.server,
            username: params.username,
            password: params.password,
        };
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let entry = platform
            .update_registry(id, request)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::UpdateRegistry)
            .id(id)
            .summary(entry.server.clone());
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(entry)
    }

    /// Delete container registry credentials by id.
    #[tool(
        description = "Delete a stored registry login by its `id`. Later pulls of private images from that server fail until a new login is stored. Needs a token with the edit scope and an admin account."
    )]
    async fn delete_registry(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        platform.delete_registry(id).await.map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::DeleteRegistry).id(id);
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(json!({ "deleted": true }))
    }

    // ==========================================
    // Group 14: Git Connections
    // ==========================================

    /// List all configured Git provider connections.
    #[tool(
        description = "List the Git provider connections (GitHub App, GitLab and Gitea OAuth apps) with their id, name, provider, URL, client id, status (`not_connected`, `connected` or `expired`) and the connected account. Secrets and tokens are never returned. An app clones private repositories through its `connection`. Takes no arguments. Read-only, but it needs an admin account or an editor without a proxy list."
    )]
    async fn list_git_connections(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let connections = platform.git_connections().await.map_err(any_error)?;
        json_result(connections)
    }

    /// Get details of a Git provider connection by id.
    #[tool(
        description = "Show one Git provider connection by its `id`: name, provider, URL, client id, status and connected account, without secrets or tokens. Read-only, but it needs an admin account or an editor without a proxy list."
    )]
    async fn get_git_connection(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let entry = platform.git_connection(id).await.map_err(any_error)?;
        json_result(entry)
    }

    /// Add a new Git provider connection (GitHub, GitLab, Gitea). Needs the edit scope.
    #[tool(
        description = "Add a GitLab or Gitea connection from an OAuth application that you registered at the provider: `name`, `provider` (`gitlab` or `gitea`), `url` (required for Gitea and for a self-hosted GitLab; gitlab.com without it), `client_id` and `client_secret`. A GitHub connection cannot be added here; it is created as a GitHub App from the panel. The new connection has the status `not_connected` until an account authorizes it in the panel. Needs a token with the edit scope and an admin account."
    )]
    async fn create_git_connection(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddGitConnectionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let name: r3v3rs3_api::container::AppName = params.name.parse().map_err(any_error)?;
        let provider: r3v3rs3_api::git_connection::GitProvider =
            params.provider.parse().map_err(any_error)?;
        let url = match params.url {
            Some(u) => Some(
                u.parse::<r3v3rs3_api::git_connection::ProviderUrl>()
                    .map_err(any_error)?,
            ),
            None => None,
        };
        let request = r3v3rs3_api::git_connection::GitConnectionRequest {
            name,
            provider,
            url,
            client_id: params.client_id,
            client_secret: Some(params.client_secret),
        };
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let entry = platform
            .add_git_connection(request, crate::clock::unix_ms())
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::AddGitConnection)
            .id(entry.id)
            .summary(format!("{} ({})", entry.name, entry.provider.as_str()));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(entry)
    }

    /// Delete a Git provider connection with its saved tokens. Needs the edit scope.
    #[tool(
        description = "Delete a Git provider connection by its `id` with its stored secret and tokens. It is refused while an app clones through it (`git_connection_in_use`); move or delete those apps first. A GitHub App stays registered on GitHub. Needs a token with the edit scope and an admin account."
    )]
    async fn delete_git_connection(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let entry = platform
            .delete_git_connection(id)
            .await
            .map_err(any_error)?;
        let record = AuditRecord::new(AuditAction::DeleteGitConnection)
            .id(id)
            .summary(format!("{} ({})", entry.name, entry.provider.as_str()));
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(json!({ "deleted": true }))
    }

    /// List repositories accessible via a Git provider connection.
    #[tool(
        description = "List the repositories that a connected Git provider connection (`id`) can read, as the provider returns them. `search` filters by name, `page` (from 1) pages through long lists. Use a result as the repository of a Git app. Read-only, but it needs an admin account or an editor without a proxy list."
    )]
    async fn list_git_repositories(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ListGitRepositoriesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let query = r3v3rs3_api::git_connection::RepositoryQuery {
            search: params.search,
            page: params.page,
        };
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let repos = platform
            .git_repositories(id, &query)
            .await
            .map_err(any_error)?;
        json_result(repos)
    }

    /// List branches of a repository accessible via a Git provider connection.
    #[tool(
        description = "List the branches of one repository (`repository` as `owner/name`) through a connected Git provider connection (`id`). Use a result as the `branch` of a Git app. Read-only, but it needs an admin account or an editor without a proxy list."
    )]
    async fn list_git_branches(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ListGitBranchesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let repo: r3v3rs3_api::git_connection::RepoName =
            params.repository.parse().map_err(any_error)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let branches = platform.git_branches(id, &repo).await.map_err(any_error)?;
        json_result(branches)
    }

    // ==========================================
    // Group 15: Accounts & Auth Tokens
    // ==========================================

    /// List all configured user accounts and their roles.
    #[tool(
        description = "List the accounts of the panel with their user name, role (`admin`, `editor` or `viewer`), proxy list (an editor or viewer limited to some proxies), whether two-factor sign-in is on, and the names of its passkeys and API tokens (never the token values). Takes no arguments. Read-only; admin accounts only."
    )]
    async fn list_accounts(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let accounts = self
            .state
            .call(&caller, GetAccountList)
            .await
            .map_err(any_error)?;
        json_result(accounts)
    }

    /// Create a new user account. Needs the edit scope.
    #[tool(
        description = "Create a panel account: `username`, `password` (at least 8 characters), `role` (`admin`, `editor` or `viewer`; default `admin`), optional `proxies` (proxy ids that limit an editor or viewer to those proxies; not allowed for an admin) and `totp` (turn on two-factor sign-in). With `totp` the answer holds the TOTP secret for the authenticator app, shown only here. Needs a token with the edit scope and an admin account."
    )]
    async fn create_account(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateAccountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let role = parse_optional_choice(
            "role",
            params.role.as_deref(),
            r3v3rs3_api::auth::Role::Admin,
        )?;
        let proxies = params.proxies.as_ref().map(parse_values).transpose()?;
        let username = params.username.clone();
        let account = r3v3rs3_api::auth::NewAccount {
            username: params.username,
            password: params.password,
            role,
            proxies,
            totp: params.totp.unwrap_or(false),
        };
        let res = self
            .state
            .call(&caller, AddAccount { account })
            .await
            .map_err(any_error)?;
        // Without TOTP the result has no fields, so the answer names the created account.
        let mut answer = serde_json::to_value(res)
            .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;
        if let Some(fields) = answer.as_object_mut() {
            fields.insert("created".into(), username.into());
        }
        json_result(answer)
    }

    /// Update an existing user account. Needs the edit scope.
    #[tool(
        description = "Change an account (`username`): `role` (`admin`, `editor` or `viewer`) is required, `password` sets a new password (omitted or empty keeps it), `proxies` replaces the proxy list (omitted removes the limit; not allowed for an admin). A change of the role, the proxy list or the password ends the sessions of the account and disables its existing API and MCP tokens. You cannot take the admin role from your own account, and at least one admin must remain. Needs a token with the edit scope and an admin account."
    )]
    async fn update_account(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateAccountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let role = parse_choice("role", &params.role)?;
        let proxies = params.proxies.as_ref().map(parse_values).transpose()?;
        let password = match params.password {
            Some(p) if !p.is_empty() => Some(p),
            _ => None,
        };
        let update = r3v3rs3_api::auth::AccountUpdate {
            role,
            proxies,
            password,
        };
        let method = UpdateAccount {
            actor: caller.username.clone(),
            username: params.username,
            update,
        };
        self.state.call(&caller, method).await.map_err(any_error)?;
        json_result(json!({ "updated": true }))
    }

    /// Delete a user account. Needs the edit scope.
    #[tool(
        description = "Delete an account (`username`) with its API tokens; its sessions stop working. You cannot delete your own account, and at least one admin must remain. Needs a token with the edit scope and an admin account."
    )]
    async fn delete_account(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DeleteAccountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let method = DeleteAccount {
            actor: caller.username.clone(),
            username: params.username,
        };
        self.state.call(&caller, method).await.map_err(any_error)?;
        json_result(json!({ "deleted": true }))
    }

    /// List API tokens belonging to the current account.
    #[tool(
        description = "List the API tokens of the account that this MCP session signed in as, with their id, name, scope (`read` or `edit`) and creation time; the token values are never returned. Takes no arguments. Read-only; any account can call it."
    )]
    async fn list_tokens(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        let accounts = self.state.accounts.borrow();
        let account = accounts.get(&caller.username).ok_or_else(|| {
            any_error(Error::AccountNotFound {
                username: caller.username.clone(),
            })
        })?;
        let tokens: Vec<r3v3rs3_api::auth::ApiTokenInfo> = account
            .tokens
            .iter()
            .map(|tok| r3v3rs3_api::auth::ApiTokenInfo {
                id: tok.id.clone(),
                name: tok.name.clone(),
                scope: tok.scope,
                created_at: tok.created_at,
            })
            .collect();
        json_result(tokens)
    }

    /// Create a persistent API token for the current account. Needs the edit scope.
    #[tool(
        description = "Create an API token for the account of this session, for scripts and the `r3` command line client: `name` labels it, `scope` is `read` (default; the token acts as a viewer) or `edit` (the token gets the role of the account). A viewer account can create only `read` tokens. The token value is returned once, in this answer only. Needs a token with the edit scope; any account can call it."
    )]
    async fn create_token(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateAuthTokenParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let scope = parse_optional_choice(
            "scope",
            params.scope.as_deref(),
            r3v3rs3_api::auth::TokenScope::Read,
        )?;
        if caller.role == r3v3rs3_api::auth::Role::Viewer
            && scope == r3v3rs3_api::auth::TokenScope::Edit
        {
            return Err(any_error(Error::Forbidden));
        }
        let result = self
            .state
            .call(
                &caller,
                CreateApiToken {
                    username: caller.username.clone(),
                    name: params.name,
                    scope,
                },
            )
            .await
            .map_err(any_error)?;
        json_result(result)
    }

    /// Delete an API token belonging to the current account. Needs the edit scope.
    #[tool(
        description = "Revoke one API token (`id` from `list_tokens`) of the account of this session; requests with it fail from now on. Needs a token with the edit scope; any account can call it."
    )]
    async fn delete_token(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        self.state
            .call(
                &caller,
                DeleteApiToken {
                    username: caller.username.clone(),
                    token_id: params.id,
                },
            )
            .await
            .map_err(any_error)?;
        json_result(json!({ "deleted": true }))
    }
}

fn parse_webhook_events(
    events: &[String],
) -> Result<Vec<r3v3rs3_api::webhook::WebhookEvent>, ErrorData> {
    events.iter().map(|e| parse_webhook_event(e)).collect()
}

/// Parses an event name such as `deploy.pending_approval`. The group may also be joined with `_`
/// (`deploy_pending_approval`): only the first `_` stands for the dot, because the names after
/// the group contain `_` themselves.
fn parse_webhook_event(name: &str) -> Result<r3v3rs3_api::webhook::WebhookEvent, ErrorData> {
    let name = name.trim();
    name.parse()
        .or_else(|err| {
            if name.contains('.') {
                Err(err)
            } else {
                name.replacen('_', ".", 1).parse()
            }
        })
        .map_err(|err: Error| ErrorData::invalid_params(err.to_string(), None))
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for McpServer {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::from_build_env())
    }

    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, ErrorData> {
        announce_call(&request.name, &context).await;
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }
}

/// Sends one notification before a tool runs. A stateless MCP request sends its response
/// headers only with the first message of the handler, and a deployment, a backup or a PITR
/// restore runs for minutes: a reverse proxy in front of the panel ends such a request once its
/// request timeout passes without headers (60 s by default). The notification starts the event
/// stream at once, and the transport then keeps it open with its keep-alive. The notification is
/// a progress one, so it goes only to a client that sent a progress token (MCP clients such as
/// Claude Code send one with every call); log messages are deprecated by the protocol. A
/// notification that cannot be sent only loses the early start, so the tool runs anyway.
async fn announce_call(tool: &str, context: &RequestContext<RoleServer>) {
    let Some(token) = context.meta.get_progress_token() else {
        return;
    };
    let param = rmcp::model::ProgressNotificationParam::new(token, 0.0)
        .with_message(format!("{tool} started"));
    if let Err(err) = context.peer.notify_progress(param).await {
        tracing::warn!(tool, %err, "the start notification of an MCP tool was not sent");
    }
}

fn apply_quarantine_overrides(
    config: &mut r3v3rs3_api::quarantine::AutoQuarantineConfig,
    params: UpdateQuarantineConfigParams,
) -> Result<(), r3v3rs3_api::error::Error> {
    if let Some(allowed_ips) = &params.allowed_ips {
        config.allowed_ips = r3v3rs3_api::cidr::parse_cidr_list(&allowed_ips.join(","))?;
    }
    apply_quarantine_limits(config, params);
    Ok(())
}

fn apply_quarantine_limits(
    config: &mut r3v3rs3_api::quarantine::AutoQuarantineConfig,
    params: UpdateQuarantineConfigParams,
) {
    if let Some(enabled) = params.enabled {
        config.enabled = enabled;
    }
    if let Some(max_violations) = params.max_violations {
        config.max_violations = max_violations;
    }
    if let Some(window_seconds) = params.window_seconds {
        config.window_seconds = window_seconds;
    }
    if let Some(ban_duration_seconds) = params.ban_duration_seconds {
        config.ban_duration_seconds = ban_duration_seconds;
    }
    if let Some(block_status_codes) = params.block_status_codes {
        config.block_status_codes = block_status_codes;
    }
    if let Some(scanner_patterns) = params.scanner_patterns {
        config.scanner_patterns = scanner_patterns;
    }
}

fn build_geoip_update_request(
    current: &r3v3rs3_api::geoip::GeoIpStatus,
    params: UpdateGeoIpConfigParams,
) -> r3v3rs3_api::geoip::UpdateGeoIpConfigRequest {
    r3v3rs3_api::geoip::UpdateGeoIpConfigRequest {
        account_id: params.account_id.or_else(|| current.account_id.clone()),
        license_key: params.license_key,
        database: params
            .database
            .map(std::path::PathBuf::from)
            .or_else(|| current.database_path.as_ref().map(std::path::PathBuf::from)),
        auto_update: params.auto_update.unwrap_or(current.auto_update),
        update_interval_days: params
            .update_interval_days
            .unwrap_or(current.update_interval_days),
    }
}

fn apply_ct_config_overrides(
    config: &mut r3v3rs3_api::ct::CtMonitorConfig,
    params: UpdateCtConfigParams,
) {
    if let Some(enabled) = params.enabled {
        config.enabled = enabled;
    }
    if let Some(interval) = params.check_interval_secs {
        config.check_interval_secs = interval;
    }
    if let Some(domains) = params.monitored_domains {
        config.monitored_domains = domains;
    }
    if let Some(auto_discover) = params.auto_discover_domains {
        config.auto_discover_domains = auto_discover;
    }
    if let Some(allowed) = params.allowed_issuers {
        config.allowed_issuers = allowed;
    }
    if let Some(alert_unknown) = params.alert_on_unknown_issuer {
        config.alert_on_unknown_issuer = alert_unknown;
    }
    if params.ct_api_url.is_some() {
        config.ct_api_url = params.ct_api_url;
    }
}

fn parse_project_env_params(
    params: UpdateProjectEnvParams,
) -> Result<(ShortId, r3v3rs3_api::platform::EnvironmentEnvSpec), ErrorData> {
    let env_id: ShortId = parse_id(&params.environment_id)?;
    let spec = r3v3rs3_api::platform::EnvironmentEnvSpec {
        variables: params.variables,
    };
    spec.validate().map_err(any_error)?;
    Ok((env_id, spec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::webhook::WebhookEvent;

    /// An agent copies the example of a tool description, so the example of `create_database`
    /// must pass the catalog check of the handler: the version is a full image tag of the engine.
    #[test]
    fn the_create_database_example_names_a_version_of_the_catalog() -> anyhow::Result<()> {
        use anyhow::Context as _;
        crate::engines::install_embedded();
        let schema = serde_json::to_value(schemars::schema_for!(CreateDatabaseParams))?;
        let text = schema["properties"]["request"]["description"]
            .as_str()
            .context("the request has no description")?;
        let example = text
            .split('`')
            .nth(1)
            .context("the description has no example")?;
        let request: r3v3rs3_api::platform::DatabaseRequest = serde_json::from_str(example)?;
        let info = r3v3rs3_api::platform::engine_info(&request.engine)
            .context("the example engine is not in the catalog")?;
        assert!(
            info.versions.contains(&request.version),
            "{} is not one of {:?}",
            request.version,
            info.versions
        );
        Ok(())
    }

    #[test]
    fn webhook_events_accept_the_underscore_form_of_the_group() {
        let cases = [
            (
                "deploy.pending_approval",
                WebhookEvent::DeployPendingApproval,
            ),
            (
                "deploy_pending_approval",
                WebhookEvent::DeployPendingApproval,
            ),
            ("system_disk_pressure", WebhookEvent::SystemDiskPressure),
            ("app_crashed", WebhookEvent::AppCrashed),
            (" test.ping ", WebhookEvent::TestPing),
        ];
        for (name, event) in cases {
            assert_eq!(parse_webhook_event(name).ok(), Some(event), "{name}");
        }
    }

    #[test]
    fn unknown_webhook_events_are_refused() {
        for name in ["deploy.pending.approval", "deploy_unknown", "", "deploy"] {
            assert!(parse_webhook_event(name).is_err(), "{name}");
        }
    }
}
