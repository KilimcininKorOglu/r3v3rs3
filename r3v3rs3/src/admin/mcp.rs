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
    AddProxy, DeleteProxy, GetProxy, GetProxyList, GetProxyStatus, PurgeProxyCache, UpdateProxy,
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
pub use rmcp::transport::streamable_http_server::tower::StreamableHttpService;

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

/// The `{ "port": {...} }` argument of `add_port`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddPortParams {
    /// The port configuration, in the same form as the body of POST /api/ports.
    pub port: serde_json::Value,
}

/// The `{ "entry": {...} }` argument of `update_port`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdatePortParams {
    /// The port entry with its id and new configuration.
    pub entry: serde_json::Value,
}

/// The argument of `set_port_early_data`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetPortEarlyDataParams {
    /// The id of the port.
    pub id: String,
    /// Whether TLS 1.3 0-RTT early data should be enabled.
    pub enabled: bool,
}

/// The argument of `update_ct_monitor_config`.
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

/// The argument of `list_ct_log_entries`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListCtEntriesParams {
    /// Optional domain filter.
    pub domain: Option<String>,
    /// Only return rogue or suspicious entries.
    pub rogue_only: Option<bool>,
    /// Maximum number of records (default: 50).
    pub limit: Option<usize>,
}

/// The argument of `check_ct_logs_now`.
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

/// The `{ "proxy": {...} }` argument of `add_proxy`, in the form of the admin API.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddProxyParams {
    /// The proxy configuration, in the same form as the body of POST /api/proxies.
    pub proxy: serde_json::Value,
}

/// The `{ "entry": {...} }` argument of `update_proxy`, in the form of the admin API.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateProxyParams {
    /// The id and the new configuration of the proxy, in the same form as the body of
    /// PUT /api/proxies/{id}.
    pub entry: serde_json::Value,
}

/// The argument of `purge_proxy_cache`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PurgeProxyCacheParams {
    /// The id of the proxy.
    pub id: String,
    /// Optional URI path prefix to purge. Empty purges the entire cache.
    pub path: Option<String>,
}

/// The argument of `set_stream_priority`.
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

/// The `{ "request": {...} }` argument of `add_acme`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddAcmeParams {
    /// The ACME certificate request configuration.
    pub request: serde_json::Value,
}

/// The `{ "list": {...} }` argument of `add_access_list`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddAccessListParams {
    /// The access control list configuration.
    pub list: serde_json::Value,
}

/// The `{ "entry": {...} }` argument of `update_access_list`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateAccessListParams {
    /// The access list entry with its id and configuration.
    pub entry: serde_json::Value,
}

/// The `{ "app": {...} }` argument of `add_app`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddAppParams {
    /// The application specification, in the same form as the body of POST /api/apps.
    pub app: serde_json::Value,
}

/// The `{ "id": "...", "app": {...} }` argument of `update_app`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateAppParams {
    /// The id of the application.
    pub id: String,
    /// The updated application specification.
    pub app: serde_json::Value,
}

/// The arguments of `get_app_logs`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AppLogsParams {
    /// The id of the application.
    pub id: String,
    /// Number of lines to tail from container logs (default: 100).
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
    pub entries: Vec<serde_json::Value>,
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
    pub entries: Option<Vec<serde_json::Value>>,
    /// Whether to overwrite existing non-empty values (default: false).
    pub overwrite: Option<bool>,
}

/// The `{ "deployment_id": "..." }` argument of `rollback_deployment`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RollbackParams {
    /// The deployment id to roll back to.
    pub deployment_id: String,
}

/// The arguments of `add_app_scheduled_command`.
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

/// The arguments of `add_webhook`.
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

/// The arguments of `update_webhook`.
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

/// The arguments of `test_webhook`.
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

/// The arguments of `add_registry`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddRegistryParams {
    /// Container registry server address (e.g. "docker.io", "ghcr.io", "registry.gitlab.com").
    pub server: String,
    /// Username or service account for authentication.
    pub username: String,
    /// Password or personal access token with pull permissions.
    pub password: Option<String>,
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

/// The arguments of `add_git_connection`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddGitConnectionParams {
    /// Distinct identifier name for the connection (e.g. "github-org", "company-gitlab").
    pub name: String,
    /// Git provider type: "github", "gitlab", or "gitea".
    pub provider: String,
    /// Optional self-hosted provider base URL (e.g. "https://gitlab.example.com"). Not used for github.
    pub url: Option<String>,
    /// OAuth application or GitHub App client ID.
    pub client_id: String,
    /// OAuth application client secret.
    pub client_secret: Option<String>,
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
    /// Database creation request (`{"name": "mydb", "engine": "postgres", "version": "17-alpine", "host_port": 5432}`).
    pub request: serde_json::Value,
}

/// The arguments of `delete_database_backup`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteBackupParams {
    /// The database app id.
    pub db_id: String,
    /// The backup id.
    pub backup_id: String,
}

/// The `{ "config": {...} }` argument of `update_backup_config`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateBackupConfigParams {
    /// Backup storage configuration (`{"cron": "0 2 * * *", "s3": {...}, ...}`).
    pub config: serde_json::Value,
}

/// The `{ "request": {...} }` argument of `add_target`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddTargetParams {
    /// Target node request (`{"name": "node-1"}`).
    pub request: serde_json::Value,
}

/// The arguments of `set_target_region`.
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
    /// Project specification (`{"name": "my-project", "description": "..."}`).
    pub spec: serde_json::Value,
}

/// The `{ "group": {...} }` argument of `create_env_group`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateEnvGroupParams {
    /// Environment group specification (`{"name": "shared-env", "entries": [...]}`).
    pub group: serde_json::Value,
}

/// The arguments of `create_dr_backup`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateDrBackupParams {
    /// Optional passphrase to encrypt the backup bundle with AES-256-GCM.
    pub passphrase: Option<String>,
    /// Whether to replicate the backup to configured S3 storage.
    pub upload_s3: Option<bool>,
}

/// The arguments of `restore_dr_backup`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RestoreDrBackupParams {
    /// Identifier of the disaster recovery backup snapshot to restore.
    pub backup_id: String,
    /// Optional passphrase to decrypt the backup bundle.
    pub passphrase: Option<String>,
}

/// The arguments of `delete_dr_backup`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteDrBackupParams {
    /// Identifier of the disaster recovery backup snapshot to delete.
    pub backup_id: String,
}

/// The arguments of `vacuum_all_logs`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct VacuumLogsParams {
    /// Optional size threshold in megabytes. Only log files exceeding this size are truncated.
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
    /// Scanner URI path substrings (e.g. ["/.env", "/.git"]).
    pub scanner_patterns: Option<Vec<String>>,
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

/// The arguments of `add_error_page_theme`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddErrorPageThemeParams {
    /// Human-friendly display name of the theme.
    pub name: String,
    /// Optional description of the theme design or purpose.
    pub description: Option<String>,
    /// List of custom error pages mapped by HTTP status code (JSON array of {status, body}).
    pub pages: Option<serde_json::Value>,
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
    pub pages: Option<serde_json::Value>,
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

/// The arguments of `add_compression_dictionary`.
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

/// The arguments of `check_grpc_health`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CheckGrpcHealthParams {
    /// The id of the proxy (optional if target_url is provided).
    pub proxy_id: Option<String>,
    /// Direct upstream target URL (e.g. `http://127.0.0.1:50051`, optional if proxy_id is provided).
    pub target_url: Option<String>,
    /// Optional gRPC service name to check. Empty string checks overall server health.
    pub service: Option<String>,
}

/// The arguments of `apply_security_header_profile`.
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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdatePrunePolicyParams {
    pub enabled: Option<bool>,
    pub interval: Option<String>,
    pub disk_pressure_threshold: Option<u8>,
    pub prune_images: Option<bool>,
    pub prune_volumes: Option<bool>,
    pub prune_build_cache: Option<bool>,
    pub prune_containers: Option<bool>,
    pub prune_logs: Option<bool>,
    pub log_vacuum_threshold_mb: Option<u64>,
    pub build_cache_keep_storage_mb: Option<u64>,
    pub kept_builds_count: Option<u32>,
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

/// Parses an id string into a typed ShortId.
fn parse_id(id: &str) -> Result<ShortId, ErrorData> {
    id.parse()
        .map_err(|err: Error| ErrorData::invalid_params(err.to_string(), None))
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

    let domains: Vec<SubjectName> = params
        .domains
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|d| d.parse().ok())
        .collect();

    let source = if store_app.is_compose() {
        let service: ServiceName = store_app
            .service
            .as_deref()
            .unwrap_or("web")
            .parse()
            .map_err(|e: Error| ErrorData::invalid_params(e.to_string(), None))?;
        AppSource::Compose {
            repository: store_app.repository.as_deref().and_then(|r| r.parse().ok()),
            branch: store_app
                .branch
                .as_deref()
                .and_then(|b| b.parse().ok())
                .unwrap_or_else(GitRef::main),
            file: store_app.file.as_deref().and_then(|f| f.parse().ok()),
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
        description = "Show the version, the profile and the runtime state of this r3v3rs3 server: the workers that serve the ports."
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

    /// The settings that the admin API can change.
    #[tool(
        description = "Show the settings of the server, for example the addresses of the ACME challenges, the notification webhooks and the discovery providers."
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
    #[tool(description = "List recent audit log entries recording administrative actions.")]
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
        description = "Get system and process performance metrics: open file descriptors, memory usage, and event loop lag."
    )]
    async fn get_system_metrics(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let _caller = caller_of(&parts)?;
        let metrics = super::system_metrics::collect_system_metrics();
        json_result(metrics)
    }

    /// Real-time system anomaly detection.
    #[tool(
        description = "Get detected real-time system anomalies, alert status, and performance thresholds."
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
    #[tool(description = "List client IP addresses currently banned by brute-force protection.")]
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
    #[tool(description = "Unban a client IP address. Needs the edit scope.")]
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
        description = "List client IP addresses currently quarantined by threat intelligence and behavioral detection."
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
        description = "Manually place a client IP address into quarantine with a reason and duration. Needs the edit scope."
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
        description = "Release a client IP address from quarantine early. Needs the edit scope."
    )]
    async fn unban_quarantined_ip(
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
        description = "Get current auto-quarantine and behavioral threat detection configuration."
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
        description = "Update and persist auto-quarantine threat detection configuration. Needs the edit scope."
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
        apply_quarantine_overrides(&mut config, params);
        let _ = super::quarantine::update_config(
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
        description = "Get the loaded GeoIP database status, file metadata, build date, and auto-update configuration."
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
        description = "Update GeoIP configuration, MaxMind license key, database path, and auto-update interval. Needs the edit scope."
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
        description = "Immediately download, verify, and hot-reload the latest GeoLite2-Country MMDB database from MaxMind. Needs the edit scope."
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
        description = "List the ports that the server listens on, with their addresses and their state."
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
    #[tool(description = "Show one port by its id, including its TLS termination settings.")]
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
    #[tool(description = "Get the socket and TLS status of a port by its id.")]
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
        description = "Create a listening port. The argument is one JSON object matching POST /api/ports. Needs the edit scope."
    )]
    async fn add_port(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddPortParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let entry: r3v3rs3_api::port::Port = serde_json::from_value(params.port)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let name = entry.name.clone();
        self.state
            .call(&caller, AddPort { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "created": name }))
    }

    /// Update an existing port configuration.
    #[tool(description = "Update an existing port configuration. Needs the edit scope.")]
    async fn update_port(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdatePortParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let entry: r3v3rs3_api::port::PortEntry = serde_json::from_value(params.entry)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        self.state
            .call(&caller, UpdatePort { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "updated": true }))
    }

    /// Delete a port by its id.
    #[tool(description = "Delete a port by its id. Needs the edit scope.")]
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
    #[tool(description = "Reset and reload the listener socket of a port. Needs the edit scope.")]
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

    /// Enable or disable TLS 1.3 0-RTT early data on a port.
    #[tool(
        description = "Enable or disable TLS 1.3 0-RTT early data with anti-replay protection on a port. Needs the edit scope."
    )]
    async fn set_port_early_data(
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
    #[tool(description = "List host network interfaces that ports can bind to.")]
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
    #[tool(description = "List the proxies with their upstream servers and their state.")]
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
    #[tool(description = "Show one proxy by its id, with its full configuration.")]
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
    #[tool(description = "Get the runtime health and upstream connection status of a proxy.")]
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
        description = "Check the health of a gRPC upstream server using the grpc.health.v1.Health/Check protocol. Specify a proxy_id or a direct target_url, and an optional gRPC service name."
    )]
    async fn check_grpc_health(
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
        description = "List all available predefined OWASP security hardening profiles (Strict, Balanced, API Only, SPA) and their associated response header rules."
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
        description = "Analyze the response headers of an HTTP proxy and return a security audit score (A+ to F), grade, and detailed OWASP risk mitigations."
    )]
    async fn analyze_proxy_security_headers(
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
        description = "Apply a predefined OWASP security hardening profile (strict, balanced, api_only, spa_frontend) to an existing HTTP proxy. Needs the edit scope."
    )]
    async fn apply_security_header_profile(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<ApplySecurityHeaderProfileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let profile = parse_mcp_security_profile(&params.profile)?;
        let id: ShortId = parse_id(&params.proxy_id)?;
        let mut entry: ProxyEntry = *self
            .state
            .call(&caller, GetProxy { id })
            .await
            .map_err(tool_error)?;

        match &mut entry.proxy.kind {
            r3v3rs3_api::proxy::ProxyKind::Http(http) => {
                let updated = r3v3rs3_api::security_headers::apply_profile_to_rules(
                    &http.headers.response,
                    profile,
                );
                http.headers.response = updated;
                let audit =
                    r3v3rs3_api::security_headers::analyze_response_rules(&http.headers.response);
                self.state
                    .call(&caller, UpdateProxy { entry })
                    .await
                    .map_err(tool_error)?;
                json_result(audit)
            }
            _ => Err(ErrorData::invalid_params(
                format!("proxy {} is not an HTTP proxy", id),
                None,
            )),
        }
    }

    /// Returns the HTTP/2 and HTTP/3 stream priority configuration of a proxy.
    #[tool(description = "Get the RFC 9218 stream priority configuration of an HTTP proxy.")]
    async fn get_stream_priority(
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
        description = "Configure RFC 9218 adaptive stream priority on an HTTP proxy. Needs the edit scope."
    )]
    async fn set_stream_priority(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<SetStreamPriorityParams>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let mut entry: ProxyEntry = *self
            .state
            .call(&caller, GetProxy { id })
            .await
            .map_err(tool_error)?;
        match &mut entry.proxy.kind {
            r3v3rs3_api::proxy::ProxyKind::Http(http) => {
                http.stream_priority.enabled = params.enabled;
                if let Some(inject) = params.inject_header {
                    http.stream_priority.inject_header = inject;
                }
                if let Some(adaptive) = params.adaptive_concurrency {
                    http.stream_priority.adaptive_concurrency = adaptive;
                }
            }
            _ => {
                return Err(ErrorData::invalid_params(
                    format!("proxy {} is not an HTTP proxy", id),
                    None,
                ));
            }
        }
        self.state
            .call(&caller, UpdateProxy { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "id": id.to_string(), "stream_priority_enabled": params.enabled }))
    }

    /// Get HTTP stream priority and adaptive concurrency statistics.
    #[tool(
        description = "Get global statistics on RFC 9218 stream priority, multiplexed urgency classes, and adaptive backpressure throttling."
    )]
    async fn get_stream_priority_stats(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let _ = caller_of(&parts)?;
        json_result(r3v3rs3_api::priority::GLOBAL_STREAM_CONTROLLER.stats())
    }

    /// A new proxy from the JSON form of the admin API.
    #[tool(
        description = "Create a proxy. The argument is one JSON object in the same form as the body of POST /api/proxies, for example {\"kind\": {\"http\": {...}}, \"ports\": [\"my-port\"], ...}. Needs the edit scope."
    )]
    async fn add_proxy(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddProxyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let proxy: Proxy = serde_json::from_value(params.proxy)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let name = proxy.name.clone();
        self.state
            .call(
                &caller,
                AddProxy {
                    entry: proxy,
                    owner: None,
                },
            )
            .await
            .map_err(tool_error)?;
        json_result(json!({ "created": name }))
    }

    /// An update of a proxy from the JSON form of the admin API.
    #[tool(
        description = "Replace the configuration of a proxy. The argument is one JSON object with the id and the new configuration, in the same form as the body of PUT /api/proxies/{id}. Needs the edit scope."
    )]
    async fn update_proxy(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateProxyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let entry: ProxyEntry = serde_json::from_value(params.entry)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        self.state
            .call(&caller, UpdateProxy { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "updated": true }))
    }

    /// A deletion of a proxy.
    #[tool(description = "Delete a proxy by its id. Needs the edit scope.")]
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
        description = "Purge cached responses for a proxy. Optional path prefix. Needs the edit scope."
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
        description = "List all pre-shared compression dictionaries (RFC 9651 / RFC 9652) configured for an HTTP proxy."
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
        description = "Get detailed configuration of a pre-shared compression dictionary by proxy id and dictionary id."
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
        description = "Add a pre-shared compression dictionary to an HTTP proxy. r3v3rs3 computes SHA-256 and serves it with Use-As-Dictionary. Needs the edit scope."
    )]
    async fn add_compression_dictionary(
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
        description = "Delete a pre-shared compression dictionary from an HTTP proxy by its id. Needs the edit scope."
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
    #[tool(description = "List the server certificates with their domains and their expiry dates.")]
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
    #[tool(description = "Get detailed information about a certificate by its id.")]
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
        description = "Check certificate coverage and wildcard auto-sharing status for domain names."
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
    #[tool(description = "Delete a certificate by its id. Needs the edit scope.")]
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
    #[tool(description = "List all ACME automated certificate requests.")]
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
    #[tool(description = "Get details of one ACME automated certificate request.")]
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
        description = "Create an ACME certificate request. The argument matches POST /api/acme. Needs the edit scope."
    )]
    async fn add_acme(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddAcmeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::acme::AcmeRequest = serde_json::from_value(params.request)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        self.state
            .call(&caller, AddAcme { request })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "created": true }))
    }

    /// Delete an ACME certificate request.
    #[tool(description = "Delete an ACME certificate request by id. Needs the edit scope.")]
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
        description = "Trigger an immediate ACME certificate order or renewal. Needs the edit scope."
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
        description = "Get CNAME delegation instructions and public DNS verification status for an ACME automated certificate request (RFC 8555 DNS alias mode)."
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
    #[tool(description = "List configured DNS provider credentials for ACME DNS-01 challenges.")]
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
    #[tool(description = "List all access control lists.")]
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
        description = "Create an access control list. Matches POST /api/access_lists. Needs the edit scope."
    )]
    async fn add_access_list(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddAccessListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let list: r3v3rs3_api::access_list::AccessList = serde_json::from_value(params.list)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        self.state
            .call(&caller, AddAccessList { list })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "created": true }))
    }

    /// Update an access control list.
    #[tool(description = "Update an access control list. Needs the edit scope.")]
    async fn update_access_list(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateAccessListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let entry: r3v3rs3_api::access_list::AccessListEntry = serde_json::from_value(params.entry)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        self.state
            .call(&caller, UpdateAccessList { entry })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "updated": true }))
    }

    /// Delete an access control list.
    #[tool(description = "Delete an access control list by id. Needs the edit scope.")]
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

    // ==========================================
    // Group 5b: Error Page Themes & Dynamic Variables
    // ==========================================

    /// List all error page themes.
    #[tool(description = "List all reusable error page themes in the centralized hub.")]
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
        json_result(themes)
    }

    /// Get an error page theme by id.
    #[tool(
        description = "Get detailed configuration of an error page theme including all mapped status code pages."
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
        description = "Create a new error page theme in the centralized pool. Needs the edit scope."
    )]
    async fn add_error_page_theme(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddErrorPageThemeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let pages = parse_theme_pages(params.pages)?;
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
    #[tool(description = "Update an existing error page theme. Needs the edit scope.")]
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
            theme.pages = parse_theme_pages(Some(pages_val))?;
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
        description = "Delete an error page theme by id. Fails if any proxy uses it. Needs the edit scope."
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
        description = "Reset error page themes pool to standard built-in themes. Needs the edit scope."
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

    /// List all PaaS applications.
    #[tool(
        description = "List all deployed PaaS applications with their status and catalog update information."
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
        let mut apps = platform.apps().await.map_err(any_error)?;
        let catalog = crate::appstore::state().catalog.clone();
        for app in &mut apps {
            app.catalog_update = super::platform::check_catalog_update(app, &catalog);
        }
        json_result(apps)
    }

    /// Get detailed configuration and status of an application.
    #[tool(description = "Get detailed configuration and status of an application by its id.")]
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
        description = "Create a new PaaS application. Matches the body of POST /api/apps. Needs the edit scope."
    )]
    async fn add_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddAppParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::platform::AppRequest = serde_json::from_value(params.app)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let app = platform
            .add_app(request, crate::clock::unix_ms())
            .await
            .map_err(any_error)?;
        json_result(app)
    }

    /// Update an application's specification.
    #[tool(description = "Update the specification of an application. Needs the edit scope.")]
    async fn update_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateAppParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let request: r3v3rs3_api::platform::AppRequest = serde_json::from_value(params.app)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let app = platform
            .update_app(id, request, crate::clock::unix_ms())
            .await
            .map_err(any_error)?;
        json_result(app)
    }

    /// Delete an application.
    #[tool(description = "Delete an application and its containers. Needs the edit scope.")]
    async fn delete_app(
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
        platform.delete_app(id).await.map_err(any_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }

    /// Trigger immediate application deployment.
    #[tool(description = "Trigger immediate deployment of an application. Needs the edit scope.")]
    async fn deploy_app(
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
        let deployment = platform
            .deploy(
                id,
                &caller.username,
                r3v3rs3_api::platform::DeploymentTrigger::Manual,
            )
            .await
            .map_err(any_error)?;
        json_result(deployment)
    }

    /// Retrieve container execution logs.
    #[tool(
        description = "Retrieve container execution logs for an application (with optional tail count and service name)."
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
    #[tool(description = "Inspect container log file sizes and storage status for an application.")]
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
        description = "Get live container resource throttling, CFS quota, memory saturation, and OOM diagnostic status for an application."
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
        description = "Get unified dashboard summary for an application including status, telemetry, resource diagnostics, deployments, and services."
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

    /// Truncate (vacuum) container log files for an application.
    #[tool(
        description = "Truncate (vacuum) container log files for an application to reclaim disk space immediately. Needs the edit scope."
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
    #[tool(description = "Get environment variables of an application.")]
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
    #[tool(description = "Update environment variables of an application. Needs the edit scope.")]
    async fn update_app_env(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateAppEnvParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let entries: Vec<r3v3rs3_api::platform::EnvEntry> =
            serde_json::from_value(serde_json::Value::Array(params.entries))
                .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        platform.set_env(id, entries).await.map_err(any_error)?;
        json_result(json!({ "updated": true }))
    }

    /// Generate cryptographically secure secrets for missing or placeholder environment variables of an application.
    #[tool(
        description = "Generate cryptographically secure secrets for missing or placeholder environment variables of an application. Needs the edit scope."
    )]
    async fn generate_app_env_secrets(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<GenerateAppEnvSecretsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let mut entries = platform.env(id).await.map_err(any_error)?;
        let overwrite = params.overwrite.unwrap_or(false);
        let save = params.save.unwrap_or(true);
        let generated_keys = r3v3rs3_api::compose_env::resolve_env_entries(
            &mut entries,
            overwrite,
            super::platform::generate_secret_for_key,
        );
        if save && !generated_keys.is_empty() {
            platform
                .set_env(id, entries.clone())
                .await
                .map_err(any_error)?;
        }
        json_result(r3v3rs3_api::compose_env::ResolveEnvTemplateResponse {
            template: String::new(),
            generated_count: generated_keys.len(),
            generated_keys,
            entries,
        })
    }

    /// Resolve an environment variable template (.env text or entries) and generate cryptographically secure secrets.
    #[tool(
        description = "Resolve a Compose or .env template and generate cryptographically secure secrets for missing or placeholder values. Needs the edit scope."
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
        let mut entries: Vec<r3v3rs3_api::platform::EnvEntry> = match params.entries {
            Some(list) => serde_json::from_value(serde_json::Value::Array(list))
                .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?,
            None => Vec::new(),
        };
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
    #[tool(description = "List deployment history and statuses for an application.")]
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
    #[tool(description = "Roll back an application to a prior deployment. Needs the edit scope.")]
    async fn rollback_deployment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<RollbackParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let dep_id: ShortId = parse_id(&params.deployment_id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let deployment = platform
            .rollback(dep_id, &caller.username)
            .await
            .map_err(any_error)?;
        json_result(deployment)
    }

    /// Check registry for updated container image.
    #[tool(description = "Check registry for updated container image. Needs the edit scope.")]
    async fn check_app_update(
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
        let status = platform.check_app_update(id).await.map_err(any_error)?;
        json_result(status)
    }

    /// List scheduled commands and cron tasks configured for an application.
    #[tool(
        description = "List scheduled commands and cron tasks configured for an application by its id."
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
        description = "Add a new scheduled cron command to an application. Needs the edit scope."
    )]
    async fn add_app_scheduled_command(
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
            .platform(&caller, Permission::Edit)
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
        description = "Update an existing scheduled cron command of an application. Needs the edit scope."
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
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let cmd = platform
            .update_scheduled_command(cmd_id, request)
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
        description = "Delete a scheduled cron command from an application. Needs the edit scope."
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
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        platform
            .delete_scheduled_command(cmd_id)
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
        description = "Run a scheduled command immediately inside the running container of an application. Needs the edit scope."
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
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let result = platform
            .execute_scheduled_command(cmd_id)
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
    #[tool(description = "View the App Store catalog of one-click deployable templates.")]
    async fn get_store(
        &self,
        Extension(_parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let response = crate::appstore::response();
        json_result(response)
    }

    /// Refresh the App Store catalog.
    #[tool(
        description = "Refresh the App Store catalog from the official repository. Needs the edit scope."
    )]
    async fn refresh_store(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let catalog = crate::appstore::fetch::fetch_catalog()
            .await
            .map_err(any_error)?;
        self.state
            .sender
            .send(crate::command::ServerCommand::SetAppStore { catalog })
            .await
            .map_err(|_| ErrorData::internal_error("failed to update catalog", None))?;
        let record =
            crate::audit::AuditRecord::new(r3v3rs3_api::audit::AuditAction::RefreshAppStore);
        self.state
            .record_audit(&caller.username, caller.client, record)
            .await;
        json_result(json!({ "refreshed": true }))
    }

    /// Install and deploy an application directly from the App Store.
    #[tool(
        description = "Install and deploy an application directly from the App Store catalog. Needs the edit scope."
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
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let app = platform
            .add_app(request, crate::clock::unix_ms())
            .await
            .map_err(any_error)?;
        let deployment = platform
            .deploy(
                app.id,
                &caller.username,
                r3v3rs3_api::platform::DeploymentTrigger::Manual,
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
    #[tool(description = "List all managed database applications.")]
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
        let dbs: Vec<_> = platform
            .apps()
            .await
            .map_err(any_error)?
            .into_iter()
            .filter(|app| app.spec.database.is_some())
            .collect();
        json_result(dbs)
    }

    /// List available database engines.
    #[tool(description = "List available database engines and version tags.")]
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
        description = "Create and deploy a new managed database with auto-generated credentials. Needs the edit scope."
    )]
    async fn create_database(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateDatabaseParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::platform::DatabaseRequest =
            serde_json::from_value(params.request)
                .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let created = super::databases::create(
            axum::extract::State(self.state.clone()),
            axum::Extension(caller),
            axum::Json(request),
        )
        .await
        .map_err(any_error)?;
        json_result(created.0)
    }

    /// List database backup snapshots.
    #[tool(description = "List backup snapshots for a managed database.")]
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

    /// Take an immediate database backup snapshot.
    #[tool(
        description = "Take an immediate backup snapshot of a managed database. Needs the edit scope."
    )]
    async fn create_database_backup(
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
        let entry = platform.run_backup(id).await.map_err(any_error)?;
        json_result(entry)
    }

    /// Delete a database backup snapshot.
    #[tool(description = "Delete a database backup snapshot. Needs the edit scope.")]
    async fn delete_database_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DeleteBackupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.db_id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        platform
            .delete_backup(id, &params.backup_id)
            .await
            .map_err(any_error)?;
        json_result(json!({ "deleted": params.backup_id }))
    }

    /// Get database backup storage configuration.
    #[tool(description = "Get database backup storage configuration (S3, SFTP, rsync).")]
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
    #[tool(description = "Update database backup storage settings. Needs the edit scope.")]
    async fn update_backup_config(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<UpdateBackupConfigParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let config: r3v3rs3_api::platform::BackupConfig = serde_json::from_value(params.config)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        platform
            .set_backup_config(config)
            .await
            .map_err(any_error)?;
        json_result(json!({ "updated": true }))
    }

    // ==========================================
    // Group 9: Agent Targets & Nodes
    // ==========================================

    /// List all Docker host targets and agent nodes.
    #[tool(description = "List all Docker host targets and agent nodes.")]
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
        description = "Add an agent target and generate enrollment token. Needs the edit scope."
    )]
    async fn add_target(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddTargetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request: r3v3rs3_api::platform::TargetRequest = serde_json::from_value(params.request)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let token = platform
            .add_target(request, crate::clock::unix_ms())
            .await
            .map_err(any_error)?;
        json_result(token)
    }

    /// Delete an agent target node.
    #[tool(description = "Delete an agent target node. Needs the edit scope.")]
    async fn delete_target(
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
        let entry = platform.delete_target(id).await.map_err(any_error)?;
        json_result(json!({ "deleted": entry.name }))
    }

    /// Set or clear the geographical region of a target node.
    #[tool(
        description = "Set or clear the geographical region of a target node. Needs the edit scope."
    )]
    async fn set_target_region(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<SetTargetRegionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let id: ShortId = parse_id(&params.id)?;
        let platform = self
            .state
            .platform(&caller, Permission::Admin)
            .await
            .map_err(any_error)?;
        let updated = platform
            .set_target_region(id, params.region)
            .await
            .map_err(any_error)?;
        json_result(updated)
    }

    /// Drain a target node for maintenance.
    #[tool(description = "Drain a target node for maintenance. Needs the edit scope.")]
    async fn drain_target(
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
        let updated = platform
            .set_target_drained(id, true)
            .await
            .map_err(any_error)?;
        json_result(updated)
    }

    /// Undrain a target node, restoring it to the active traffic pool.
    #[tool(
        description = "Undrain a target node, restoring it to the active traffic pool. Needs the edit scope."
    )]
    async fn undrain_target(
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
        let updated = platform
            .set_target_drained(id, false)
            .await
            .map_err(any_error)?;
        json_result(updated)
    }

    // ==========================================
    // Group 10: Projects & Environment Variable Groups
    // ==========================================

    /// List all platform projects.
    #[tool(description = "List all platform projects.")]
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
    #[tool(description = "Create a new project. Needs the edit scope.")]
    async fn create_project(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateProjectParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let spec: r3v3rs3_api::platform::ProjectSpec = serde_json::from_value(params.spec)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let project = platform
            .create_project(spec, crate::clock::unix_ms())
            .await
            .map_err(any_error)?;
        json_result(project)
    }

    /// Delete a project by id.
    #[tool(description = "Delete a project by id. Needs the edit scope.")]
    async fn delete_project(
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
        let project = platform.delete_project(id).await.map_err(any_error)?;
        json_result(project)
    }

    /// List all shared environment variable groups.
    #[tool(description = "List all shared environment variable groups.")]
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
    #[tool(description = "Create a shared environment variable group. Needs the edit scope.")]
    async fn create_env_group(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateEnvGroupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let group: r3v3rs3_api::platform::EnvGroupSpec = serde_json::from_value(params.group)
            .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
        let platform = self
            .state
            .platform(&caller, Permission::Edit)
            .await
            .map_err(any_error)?;
        let created = platform
            .create_env_group(group, crate::clock::unix_ms())
            .await
            .map_err(any_error)?;
        json_result(created)
    }

    /// Delete an environment variable group by id.
    #[tool(description = "Delete an environment variable group by id. Needs the edit scope.")]
    async fn delete_env_group(
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
        let deleted = platform.delete_env_group(id).await.map_err(any_error)?;
        json_result(deleted)
    }

    // ==========================================
    // Group 11: Disaster Recovery
    // ==========================================

    /// List all full disaster recovery backup snapshots.
    #[tool(description = "List all full disaster recovery backup snapshots.")]
    async fn list_dr_backups(
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
        description = "Create a full system disaster recovery backup snapshot. Needs the edit scope."
    )]
    async fn create_dr_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateDrBackupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request = r3v3rs3_api::platform::CreateDisasterRecoveryRequest {
            passphrase: params.passphrase,
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
        description = "Restore a full system disaster recovery backup snapshot by ID. Needs the edit scope."
    )]
    async fn restore_dr_backup(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<RestoreDrBackupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
        let request = r3v3rs3_api::platform::RestoreDisasterRecoveryRequest {
            passphrase: params.passphrase,
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
        description = "Delete a full system disaster recovery backup snapshot by ID. Needs the edit scope."
    )]
    async fn delete_dr_backup(
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

    /// Truncate (vacuum) container logs across the platform.
    #[tool(
        description = "Truncate (vacuum) container logs across the platform exceeding the optional MB threshold. Needs the edit scope."
    )]
    async fn vacuum_all_logs(
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
        description = "Execute Docker disk hygiene prune immediately to remove dangling images, stopped containers, build caches, and old logs according to the configured policy. Needs the edit scope."
    )]
    async fn run_platform_prune(
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
        description = "Get the Docker disk hygiene and garbage collector prune policy including interval, disk pressure threshold, build cache retention, and image cleanup settings. Needs the read scope."
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
        description = "Update the Docker disk hygiene and garbage collector prune policy: enabled, interval, disk_pressure_threshold, build_cache_keep_storage_mb, kept_builds_count, etc. Needs the edit scope."
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
        description = "Get host node resource threshold alert rules (disk space, bandwidth spike, file descriptor exhaustion, and socket exhaustion). Needs the read scope."
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
        description = "Update host node resource threshold alert rules (disk space, bandwidth spike, file descriptor exhaustion, and socket exhaustion). Needs the edit scope."
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
        description = "List all active long-lived connections and streams (WebSocket, TCP, SSE) with protocol, client IP, target, duration, transferred bytes, and instantaneous throughput."
    )]
    async fn list_active_connections(
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
        description = "Immediately terminate an active long-lived connection or stream by its connection ID. Needs the edit scope."
    )]
    async fn terminate_active_connection(
        &self,
        Parameters(params): Parameters<IdParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
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
        description = "Get the Certificate Transparency (CT) log monitor configuration, including monitored domains, check interval, allowed CAs, and rogue alert settings."
    )]
    async fn get_ct_monitor_config(
        &self,
        Extension(_parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = crate::certs::ct::get_or_init_state(&config_dir);
        json_result(monitor.get_config())
    }

    /// Update the Certificate Transparency (CT) monitor configuration.
    #[tool(
        description = "Update the Certificate Transparency (CT) log monitor configuration. Needs the edit scope."
    )]
    async fn update_ct_monitor_config(
        &self,
        Parameters(params): Parameters<UpdateCtConfigParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = crate::certs::ct::get_or_init_state(&config_dir);
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
        description = "Get the runtime status of the Certificate Transparency monitor: last check time, next scheduled run, active rogue alerts count, and total entries tracked."
    )]
    async fn get_ct_monitor_status(
        &self,
        Extension(_parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = crate::certs::ct::get_or_init_state(&config_dir);
        json_result(monitor.get_status())
    }

    /// List Certificate Transparency log entries and rogue certificate alerts.
    #[tool(
        description = "List discovered Certificate Transparency log entries and rogue certificate alerts for monitored domains, with optional domain filter and rogue-only filter."
    )]
    async fn list_ct_log_entries(
        &self,
        Parameters(params): Parameters<ListCtEntriesParams>,
        Extension(_parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = crate::certs::ct::get_or_init_state(&config_dir);
        let query = r3v3rs3_api::ct::CtEntriesQuery {
            domain: params.domain,
            rogue_only: params.rogue_only,
            limit: params.limit,
        };
        json_result(monitor.list_entries(query))
    }

    /// Trigger an immediate on-demand check of Certificate Transparency logs.
    #[tool(
        description = "Trigger an immediate on-demand query of public Certificate Transparency logs for a specific domain or all monitored domains. Needs the edit scope."
    )]
    async fn check_ct_logs_now(
        &self,
        Parameters(params): Parameters<CheckCtParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = crate::certs::ct::get_or_init_state(&config_dir);
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
        description = "Dismiss or acknowledge a Certificate Transparency rogue certificate alert by its ID. Needs the edit scope."
    )]
    async fn dismiss_ct_alert(
        &self,
        Parameters(params): Parameters<DismissCtAlertParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        require_edit(&parts)?;
        let caller = caller_of(&parts)?;
        let config_dir = self.state.data.lock().await.app_info.config_path.clone();
        let monitor = crate::certs::ct::get_or_init_state(&config_dir);
        let status = match params.status.as_deref() {
            Some("investigating") => Some(r3v3rs3_api::ct::CtAlertStatus::Investigating),
            Some("confirmed_rogue") => Some(r3v3rs3_api::ct::CtAlertStatus::ConfirmedRogue),
            _ => Some(r3v3rs3_api::ct::CtAlertStatus::Dismissed),
        };
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
    #[tool(description = "List all configured outbound notification webhooks.")]
    async fn list_webhooks(
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
    #[tool(description = "Get details of an outbound webhook by its id.")]
    async fn get_webhook(
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
        description = "Create a new outbound notification webhook subscribing to platform events. Needs the edit scope."
    )]
    async fn add_webhook(
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
        description = "Update an existing outbound notification webhook configuration. Needs the edit scope."
    )]
    async fn update_webhook(
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
        description = "Delete an outbound notification webhook by its id. Needs the edit scope."
    )]
    async fn delete_webhook(
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
        description = "Send a test notification payload to an outbound webhook by id or target URL. Needs the edit scope."
    )]
    async fn test_webhook(
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
        let event = match params.event {
            Some(ref e) => Some(
                e.trim()
                    .replace('_', ".")
                    .parse::<r3v3rs3_api::webhook::WebhookEvent>()
                    .or_else(|_| e.trim().parse::<r3v3rs3_api::webhook::WebhookEvent>())
                    .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?,
            ),
            None => None,
        };
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
    #[tool(description = "List all configured container registries for image pulls.")]
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
        description = "Add container registry credentials for private image pulls. Needs the edit scope."
    )]
    async fn add_registry(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AddRegistryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller_of(&parts)?;
        require_edit(&parts)?;
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
    #[tool(description = "Update container registry credentials by id. Needs the edit scope.")]
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
    #[tool(description = "Delete container registry credentials by id. Needs the edit scope.")]
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
    #[tool(description = "List all configured Git provider connections (GitHub, GitLab, Gitea).")]
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
    #[tool(description = "Get details of a Git provider connection by id.")]
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
        description = "Add a new Git provider connection (GitHub, GitLab, Gitea). Needs the edit scope."
    )]
    async fn add_git_connection(
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
            client_secret: params.client_secret,
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
        description = "Delete a Git provider connection with its saved tokens by id. Needs the edit scope."
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
    #[tool(description = "List repositories accessible through a Git provider connection.")]
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
        description = "List branches of a repository accessible through a Git provider connection."
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
}

fn parse_webhook_events(
    events: &[String],
) -> Result<Vec<r3v3rs3_api::webhook::WebhookEvent>, ErrorData> {
    events
        .iter()
        .map(|e| {
            e.trim()
                .replace('_', ".")
                .parse::<r3v3rs3_api::webhook::WebhookEvent>()
                .or_else(|_| e.trim().parse::<r3v3rs3_api::webhook::WebhookEvent>())
                .map_err(|err| ErrorData::invalid_params(err.to_string(), None))
        })
        .collect()
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for McpServer {}

fn apply_quarantine_overrides(
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

fn parse_theme_pages(
    pages_val: Option<serde_json::Value>,
) -> Result<Vec<r3v3rs3_api::proxy::CustomErrorPage>, ErrorData> {
    match pages_val {
        Some(val) => serde_json::from_value(val)
            .map_err(|e| ErrorData::invalid_params(format!("invalid pages array: {e}"), None)),
        None => Ok(Vec::new()),
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
