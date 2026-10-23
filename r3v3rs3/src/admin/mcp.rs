//! The Model Context Protocol server of the admin API.
//!
//! Every tool runs through the same RPC methods that the admin API handlers use, so the
//! permission checks, the proxy scopes and the audit log of the server apply unchanged. The
//! tools answer with pretty-printed JSON, because the schemas of the API types are larger than
//! a tool argument needs to be.

use super::{AppState, McpScope};
use crate::accounts::{Caller, Permission};
use crate::server::rpc::access_lists::{
    AddAccessList, DeleteAccessList, GetAccessLists, UpdateAccessList,
};
use crate::server::rpc::acme::{AddAcme, DeleteAcme, GetAcme, GetAcmeList, RequestAcme};
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
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry};
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

/// The `{ "deployment_id": "..." }` argument of `rollback_deployment`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RollbackParams {
    /// The deployment id to roll back to.
    pub deployment_id: String,
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
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for McpServer {}
