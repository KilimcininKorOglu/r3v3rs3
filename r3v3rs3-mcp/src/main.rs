//! A command line MCP client for the r3v3rs3 admin API.
//!
//! `login` runs the OAuth 2.1 authorization code flow in the browser and stores the tokens.
//! `tools` and `call` connect to `/api/mcp` with the stored access token.

mod oauth;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use rmcp::model::CallToolRequestParams;
use rmcp::service::serve_client;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "r3v3rs3-mcp",
    about = "An MCP client for the r3v3rs3 admin API"
)]
struct Args {
    /// The origin of the admin API, for example http://127.0.0.1:46492.
    server: String,

    /// An access token instead of the stored tokens, for scripts. The token must carry the
    /// scopes that the tool needs.
    #[arg(long, env = "R3V3RS3_MCP_TOKEN", hide_env_values = true)]
    token: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Runs the OAuth authorization flow in the browser and stores the tokens.
    Login,
    /// Lists the tools that the server exposes.
    Tools,
    /// Calls one tool. The arguments are one JSON object, for example '{"id": "abc"}'.
    Call {
        name: String,
        #[clap(default_value = "{}")]
        arguments: String,
    },
}

/// A silent MCP client: this program prints the results itself.
#[derive(Clone, Default)]
struct QuietClient;

impl rmcp::ClientHandler for QuietClient {}

fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build the HTTP client")
}

/// The origin of the server URL, without a trailing slash.
fn origin(server: &str) -> anyhow::Result<String> {
    let url: url::Url = server
        .parse()
        .map_err(|err| anyhow::anyhow!("invalid server URL: {err}"))?;
    Ok(url.origin().ascii_serialization())
}

async fn connect(
    server: &str,
    access_token: &str,
) -> anyhow::Result<rmcp::service::RunningService<rmcp::service::RoleClient, QuietClient>> {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("{server}/api/mcp"))
            // The transport adds the `Bearer ` prefix itself.
            .auth_header(access_token),
    );
    serve_client(QuietClient, transport)
        .await
        .map_err(|err| anyhow::anyhow!("the MCP handshake failed: {err}"))
}

/// The explicit `--token` of the command, or the stored tokens refreshed when needed.
async fn access_token_of(
    client: &reqwest::Client,
    server: &str,
    token: Option<String>,
) -> anyhow::Result<String> {
    match token {
        Some(token) => Ok(token),
        None => Ok(oauth::valid_tokens(client, server).await?.access_token),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // The reqwest client of the transport builds on rustls without a default provider.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install the rustls crypto provider");
    let args = Args::parse();
    let server = origin(&args.server)?;
    let client = http_client()?;
    match args.command {
        Command::Login => login(&client, &server).await?,
        Command::Tools => tools(&client, &server, args.token).await?,
        Command::Call { name, arguments } => {
            call(&client, &server, args.token, &name, &arguments).await?
        }
    }
    Ok(())
}

/// Runs the OAuth flow and stores the tokens.
async fn login(client: &reqwest::Client, server: &str) -> anyhow::Result<()> {
    let tokens = oauth::login(client, server, oauth::SCOPES).await?;
    println!(
        "Signed in to {server}. The tokens are in {}.",
        oauth::token_path(server)?.display()
    );
    println!(
        "The access token is valid until the Unix time {}.",
        tokens.expires_at
    );
    Ok(())
}

/// Prints one line per tool with its description.
async fn tools(
    client: &reqwest::Client,
    server: &str,
    token: Option<String>,
) -> anyhow::Result<()> {
    let access_token = access_token_of(client, server, token).await?;
    let mcp = connect(server, &access_token).await?;
    let tools = mcp.list_tools(Default::default()).await?;
    for tool in &tools.tools {
        println!(
            "{}\n    {}",
            tool.name,
            tool.description.as_deref().unwrap_or("")
        );
    }
    mcp.cancel().await?;
    Ok(())
}

/// Calls one tool and prints its text content.
async fn call(
    client: &reqwest::Client,
    server: &str,
    token: Option<String>,
    name: &str,
    arguments: &str,
) -> anyhow::Result<()> {
    let access_token = access_token_of(client, server, token).await?;
    let value: serde_json::Value = serde_json::from_str(arguments)
        .map_err(|err| anyhow::anyhow!("the arguments are not one JSON object: {err}"))?;
    let Some(arguments) = value.as_object() else {
        bail!("the arguments must be one JSON object");
    };
    let mcp = connect(server, &access_token).await?;
    let mut params = CallToolRequestParams::new(name.to_string());
    params.arguments = Some(arguments.clone());
    let result = mcp
        .call_tool(params)
        .await
        .map_err(|err| anyhow::anyhow!("the tool call failed: {err}"))?;
    for content in &result.content {
        if let Some(text) = content.as_text() {
            println!("{}", text.text);
        }
    }
    let errored = result.is_error == Some(true);
    mcp.cancel().await?;
    if errored {
        bail!("the tool answered with an error");
    }
    Ok(())
}
