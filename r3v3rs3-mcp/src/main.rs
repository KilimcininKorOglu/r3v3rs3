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
use sha2::Digest;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "r3v3rs3-mcp",
    about = "An MCP client for the r3v3rs3 admin API"
)]
struct Args {
    /// The origin of the admin API, for example http://127.0.0.1:46492. Every command
    /// except `update` needs it.
    server: Option<String>,

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
    /// Installs the newest release of this client over the running binary.
    Update,
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
    let client = http_client()?;
    match args.command {
        Command::Update => update(&client).await?,
        _ => {
            let server = origin(args.server.as_deref().context(
                "this command needs the server origin, for example http://127.0.0.1:46492",
            )?)?;
            match args.command {
                Command::Login => login(&client, &server).await?,
                Command::Tools => tools(&client, &server, args.token).await?,
                Command::Call { name, arguments } => {
                    call(&client, &server, args.token, &name, &arguments).await?
                }
                Command::Update => unreachable!("handled above"),
            }
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

/// The GitHub repository that ships the releases of this client.
const RELEASES_API: &str = "https://api.github.com/repos/KilimcininKorOglu/r3v3rs3/releases/latest";

/// The release archive of this platform. The Linux archives hold the static musl build,
/// so they run on every distribution regardless of the installed libc.
fn asset_name() -> anyhow::Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("r3v3rs3-mcp-x86_64-unknown-linux-musl.tar.gz"),
        ("linux", "aarch64") => Ok("r3v3rs3-mcp-aarch64-unknown-linux-musl.tar.gz"),
        ("macos", "x86_64") => Ok("r3v3rs3-mcp-x86_64-apple-darwin.tar.gz"),
        ("macos", "aarch64") => Ok("r3v3rs3-mcp-aarch64-apple-darwin.tar.gz"),
        ("windows", "x86_64") => Ok("r3v3rs3-mcp-x86_64-pc-windows-msvc.zip"),
        (os, arch) => bail!("no release archive exists for {os}/{arch}"),
    }
}

/// The name of the executable inside the archive of this platform.
fn binary_name() -> &'static str {
    if std::env::consts::OS == "windows" {
        "r3v3rs3-mcp.exe"
    } else {
        "r3v3rs3-mcp"
    }
}

/// Compares two `major.minor.patch` version strings without a semver crate.
fn newer_than(latest: &str, current: &str) -> Option<bool> {
    let parse = |text: &str| -> Option<Vec<u64>> {
        text.split('.')
            .map(|part| part.parse().ok())
            .collect::<Option<Vec<_>>>()
    };
    let (latest, current) = (parse(latest)?, parse(current)?);
    Some(latest > current)
}

/// The `tag_name` of the newest release, without its `v` prefix.
async fn latest_version(client: &reqwest::Client) -> anyhow::Result<String> {
    let release: serde_json::Value = client
        .get(RELEASES_API)
        .header(reqwest::header::USER_AGENT, "r3v3rs3-mcp")
        .send()
        .await
        .context("the release API did not answer")?
        .error_for_status()
        .context("the release API answered with an error")?
        .json()
        .await
        .context("the release API did not answer with JSON")?;
    let tag = release["tag_name"]
        .as_str()
        .context("the release API named no tag")?;
    Ok(tag.trim_start_matches('v').to_string())
}

/// Reads the sha256 digest of `asset` out of the SHA256SUMS file of the release.
fn digest_of(sums: &str, asset: &str) -> anyhow::Result<String> {
    for line in sums.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(digest), Some(name)) = (parts.next(), parts.next()) {
            // The sum file lists the names as `./r3v3rs3-mcp-...`.
            if name.trim_start_matches("./") == asset {
                return Ok(digest.to_string());
            }
        }
    }
    bail!("{asset} is not in the checksums of the release")
}

/// Installs the newest release of this client over the running binary.
async fn update(client: &reqwest::Client) -> anyhow::Result<()> {
    let asset = asset_name()?;
    let latest = latest_version(client).await?;
    let current = env!("CARGO_PKG_VERSION");
    match newer_than(&latest, current) {
        Some(false) => {
            println!("r3v3rs3-mcp {current} is the newest release.");
            return Ok(());
        }
        None => bail!("the release version {latest} is not three numbers"),
        Some(true) => {}
    }
    println!("Downloading r3v3rs3-mcp {latest}...");
    let base = format!("https://github.com/KilimcininKorOglu/r3v3rs3/releases/download/v{latest}");
    let sums = download(client, &format!("{base}/SHA256SUMS")).await?;
    let archive = download(client, &format!("{base}/{asset}")).await?;
    let sums = String::from_utf8(sums).context("the checksums are not UTF-8 text")?;
    let expected = digest_of(&sums, asset)?;
    let actual = hex::encode(sha2::Sha256::digest(&archive));
    if actual != expected {
        bail!("the sha256 digest of {asset} does not match the checksums of the release");
    }
    install(&archive)?;
    println!("r3v3rs3-mcp {latest} is installed. The next run uses it.");
    Ok(())
}

/// Downloads one release file as bytes.
async fn download(client: &reqwest::Client, url: &str) -> anyhow::Result<Vec<u8>> {
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, "r3v3rs3-mcp")
        .send()
        .await
        .with_context(|| format!("{url} did not answer"))?
        .error_for_status()
        .with_context(|| format!("{url} answered with an error"))?
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .with_context(|| format!("{url} could not be read"))
}

/// Extracts the binary from the archive and replaces the running file atomically.
fn install(archive: &[u8]) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("the path of the running binary is unknown")?;
    let dir = exe
        .parent()
        .context("the running binary has no directory")?
        .to_path_buf();
    let temporary = dir.join(format!(".r3v3rs3-mcp.new.{}", std::process::id()));
    let bytes = unpack(archive)?;
    std::fs::write(&temporary, &bytes)
        .context("the binary could not be written next to the running one")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))
            .context("the new binary could not be made executable")?;
    }
    // A rename over the running binary is allowed on Unix, and the old file keeps serving
    // this process until it exits. Windows locks the running file, so the old binary
    // moves aside first and the next update removes it.
    if std::fs::rename(&temporary, &exe).is_err() {
        let aside = dir.join("r3v3rs3-mcp.old");
        let _ = std::fs::remove_file(&aside);
        std::fs::rename(&exe, &aside).context("the old binary could not be moved aside")?;
        std::fs::rename(&temporary, &exe).context("the running binary could not be replaced")?;
    }
    Ok(())
}

/// Reads the executable out of the tar.gz or the zip archive of this platform.
fn unpack(archive: &[u8]) -> anyhow::Result<Vec<u8>> {
    let name = binary_name();
    if std::env::consts::OS == "windows" {
        let reader = std::io::Cursor::new(archive);
        let mut zip = zip::ZipArchive::new(reader).context("the archive is not readable")?;
        let mut file = zip
            .by_name(name)
            .context("the archive holds no r3v3rs3-mcp binary")?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes)
            .context("the archive could not be unpacked")?;
        return Ok(bytes);
    }
    let reader = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(reader);
    for entry in tar.entries().context("the archive is not readable")? {
        let mut entry = entry.context("the archive is not readable")?;
        if entry
            .path()
            .map_err(|err| anyhow::anyhow!("{err}"))?
            .to_string_lossy()
            == name
        {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes)
                .context("the archive could not be unpacked")?;
            return Ok(bytes);
        }
    }
    bail!("the archive holds no r3v3rs3-mcp binary")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_version_is_detected_by_number_not_by_text() {
        assert_eq!(newer_than("1.7.4", "1.7.3"), Some(true));
        assert_eq!(newer_than("1.8.0", "1.7.99"), Some(true));
        assert_eq!(newer_than("1.7.3", "1.7.3"), Some(false));
        assert_eq!(newer_than("1.7.2", "1.7.10"), Some(false));
        assert_eq!(newer_than("unknown", "1.7.3"), None);
    }

    #[test]
    fn the_digest_is_read_from_the_sum_file_with_or_without_a_dot_prefix() {
        let sums = "abc123  ./r3v3rs3-mcp-x86_64-unknown-linux-musl.tar.gz\ndef456  SHA256SUMS\n";
        assert_eq!(
            digest_of(sums, "r3v3rs3-mcp-x86_64-unknown-linux-musl.tar.gz").unwrap(),
            "abc123"
        );
        assert!(digest_of(sums, "r3v3rs3-mcp-aarch64-unknown-linux-musl.tar.gz").is_err());
    }
}
