//! The Model Context Protocol server of the admin API.
//!
//! Every tool runs through the same RPC methods that the admin API handlers use, so the
//! permission checks, the proxy scopes and the audit log of the server apply unchanged. The
//! tools answer with pretty-printed JSON, because the schemas of the API types are larger than
//! a tool argument needs to be.

use super::{AppState, McpScope};
use crate::accounts::Caller;
use crate::server::rpc::certs::GetCertList;
use crate::server::rpc::cluster::GetClusterStatus;
use crate::server::rpc::config::GetConfig;
use crate::server::rpc::ports::{GetPort, GetPortList};
use crate::server::rpc::proxies::{AddProxy, DeleteProxy, GetProxy, GetProxyList, UpdateProxy};
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
    /// The id of the port, proxy or certificate.
    pub id: String,
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

/// The text result of a tool, as pretty JSON.
fn json_result(value: impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(&value)
        .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

#[tool_router]
impl McpServer {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    /// The version, the target and the state of the server.
    #[tool(
        description = "Show the version, the profile and the runtime state of this r3v3rs3 server: the workers that serve the ports and the cluster status."
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
        let cluster: r3v3rs3_api::cluster::ClusterStatus = *self
            .state
            .call(&caller, GetClusterStatus)
            .await
            .map_err(tool_error)?;
        json_result(json!({
            "app": app_info,
            "workers": workers,
            "cluster": cluster,
        }))
    }

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
        let id: ShortId = params
            .id
            .parse()
            .map_err(|err: r3v3rs3_api::error::Error| {
                ErrorData::invalid_params(err.to_string(), None)
            })?;
        let port: r3v3rs3_api::port::PortEntry = *self
            .state
            .call(&caller, GetPort { id })
            .await
            .map_err(tool_error)?;
        json_result(port)
    }

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
        let id: ShortId = params
            .id
            .parse()
            .map_err(|err: r3v3rs3_api::error::Error| {
                ErrorData::invalid_params(err.to_string(), None)
            })?;
        let proxy: ProxyEntry = *self
            .state
            .call(&caller, GetProxy { id })
            .await
            .map_err(tool_error)?;
        json_result(proxy)
    }

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
        let id: ShortId = params
            .id
            .parse()
            .map_err(|err: r3v3rs3_api::error::Error| {
                ErrorData::invalid_params(err.to_string(), None)
            })?;
        self.state
            .call(&caller, DeleteProxy { id })
            .await
            .map_err(tool_error)?;
        json_result(json!({ "deleted": id.to_string() }))
    }
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for McpServer {}
