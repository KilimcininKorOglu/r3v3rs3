//! A command line client for the r3v3rs3 admin API.
//!
//! `login` runs the OAuth 2.1 authorization code flow in the browser and stores the tokens.

mod api_client;
mod oauth;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use sha2::Digest;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "r3",
    bin_name = "r3",
    about = "A command line client for the r3v3rs3 admin API"
)]
struct Args {
    /// The origin of the admin API, for example http://127.0.0.1:46492. Every command
    /// except `update` needs it.
    server: Option<String>,

    /// An access token instead of the stored tokens, for scripts.
    #[arg(long, env = "R3V3RS3_TOKEN", hide_env_values = true)]
    token: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Runs the OAuth authorization flow in the browser and stores the tokens.
    Login,
    /// Manage listening ports.
    #[command(subcommand)]
    Port(PortCommand),
    /// Manage reverse proxies.
    #[command(subcommand)]
    Proxy(ProxyCommand),
    /// Inspect live HTTP traffic and long-lived connections.
    #[command(subcommand)]
    Traffic(TrafficCommand),
    /// Manage TLS/SSL certificates.
    #[command(subcommand)]
    Cert(CertCommand),
    /// Manage ACME automatic certificate orders.
    #[command(subcommand)]
    Acme(AcmeCommand),
    /// Manage IP/CIDR and country access lists.
    #[command(subcommand)]
    Acl(AclCommand),
    /// Manage error page themes and dynamic template variables.
    #[command(subcommand)]
    ErrorPageTheme(ErrorPageThemeCommand),
    /// Manage DNS provider credentials for DNS-01 ACME.
    #[command(subcommand)]
    Dns(DnsCommand),
    /// Manage deployed applications.
    #[command(subcommand)]
    App(AppCommand),
    /// Manage database services.
    #[command(subcommand)]
    Db(DbCommand),
    /// Manage database backups.
    #[command(subcommand)]
    Backup(BackupCommand),
    /// Manage multi-node agent targets.
    #[command(subcommand)]
    Target(TargetCommand),
    /// Manage projects and shared environment variables.
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Browse and install applications from the App Store catalog.
    #[command(subcommand)]
    Store(StoreCommand),
    /// Manage security rules and banned IP addresses.
    #[command(subcommand)]
    Security(SecurityCommand),
    /// Manage outbound webhook integrations.
    #[command(subcommand)]
    Webhook(WebhookCommand),
    /// Manage scheduled background cron commands for applications.
    #[command(subcommand)]
    Cron(CronCommand),
    /// Manage server settings and configuration backup/restore.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Manage platform operations and system metrics.
    #[command(subcommand)]
    Platform(PlatformCommand),
    /// Apply a declarative YAML or JSON manifest to configure ports, proxies, and apps.
    Apply {
        /// Path to manifest file, or '-' to read from standard input.
        #[arg(short = 'f', long = "file")]
        file: String,
    },
    /// Installs the newest release of this client over the running binary.
    Update,
}

#[derive(Debug, Subcommand)]
pub enum PortCommand {
    /// Lists all configured ports.
    #[command(alias = "ls")]
    List,
    /// Shows configuration of a port.
    Get { id: String },
    /// Adds a port from inline JSON/YAML, a file path, or stdin ('-').
    Add { payload: String },
    /// Deletes a port.
    #[command(alias = "rm")]
    Delete { id: String },
    /// Resets a port listener.
    Reset { id: String },
}

#[derive(Debug, Subcommand)]
pub enum ProxyCommand {
    /// Lists all proxies.
    #[command(alias = "ls")]
    List,
    /// Shows configuration of a proxy.
    Get { id: String },
    /// Adds a proxy from inline JSON/YAML, a file path, or stdin ('-').
    Add { payload: String },
    /// Shows live status of a proxy.
    Status { id: String },
    /// Deletes a proxy.
    #[command(alias = "rm")]
    Delete { id: String },
    /// Purges cache for a proxy.
    PurgeCache {
        id: String,
        #[arg(short, long)]
        path: Option<String>,
    },
    /// Checks gRPC upstream health status using grpc.health.v1.Health/Check protocol.
    GrpcHealth {
        id: String,
        /// Optional service name to check. Empty string checks overall server health.
        #[arg(short, long)]
        service: Option<String>,
        /// Optional direct target URL override.
        #[arg(short, long)]
        target_url: Option<String>,
    },
    /// Manages pre-shared compression dictionaries (RFC 9651 / RFC 9652) of an HTTP proxy.
    #[command(subcommand)]
    Dictionary(ProxyDictionaryCommand),
    /// Audits and hardens HTTP response security headers (OWASP/HSTS/CSP).
    #[command(subcommand)]
    SecurityHeaders(ProxySecurityHeadersCommand),
}

#[derive(Debug, Subcommand)]
pub enum ProxySecurityHeadersCommand {
    /// Shows the security scorecard and missing headers of an HTTP proxy.
    Audit { id: String },
    /// Applies a predefined OWASP security profile (strict, balanced, api-only, spa) to a proxy.
    Apply {
        id: String,
        /// Predefined profile: `strict`, `balanced`, `api-only`, or `spa`.
        profile: String,
    },
    /// Lists all available predefined OWASP security hardening profiles.
    #[command(alias = "ls")]
    Profiles,
}

#[derive(Debug, Subcommand)]
pub enum ProxyDictionaryCommand {
    /// Lists all compression dictionaries of a proxy.
    #[command(alias = "ls")]
    List { proxy_id: String },
    /// Shows a compression dictionary by proxy id and dictionary id.
    Get { proxy_id: String, dict_id: String },
    /// Adds a compression dictionary to an HTTP proxy.
    Add {
        proxy_id: String,
        #[arg(short, long)]
        name: String,
        #[arg(short, long)]
        match_pattern: String,
        #[arg(short, long)]
        path: String,
        /// Raw dictionary data or @file path.
        #[arg(short, long)]
        data: String,
    },
    /// Deletes a compression dictionary from a proxy.
    #[command(alias = "rm")]
    Delete { proxy_id: String, dict_id: String },
}

#[derive(Debug, Subcommand)]
pub enum TrafficCommand {
    /// Manage active long-lived connections and streams.
    #[command(subcommand)]
    Connections(ConnectionsCommand),
}

#[derive(Debug, Subcommand)]
pub enum ConnectionsCommand {
    /// Lists active long-lived connections and streams.
    #[command(alias = "ls")]
    List {
        /// Optional proxy ID filter.
        #[arg(short, long)]
        proxy: Option<String>,
    },
    /// Terminates an active connection by its unique ID.
    #[command(alias = "kill", alias = "terminate")]
    Close {
        /// Active connection ID.
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum CertCommand {
    /// Lists all TLS certificates.
    #[command(alias = "ls")]
    List,
    /// Shows details of a certificate.
    Get { id: String },
    /// Uploads a certificate and private key.
    Upload {
        #[arg(long)]
        cert: String,
        #[arg(long)]
        key: String,
        #[arg(long)]
        kind: Option<String>,
    },
    /// Deletes a certificate.
    #[command(alias = "rm")]
    Delete { id: String },
    /// Checks domain certificate coverage.
    Coverage,
}

#[derive(Debug, Subcommand)]
pub enum AcmeCommand {
    /// Lists all ACME certificate orders.
    #[command(alias = "ls")]
    List,
    /// Shows details of an ACME order.
    Get { id: String },
    /// Adds an ACME order from inline JSON/YAML, a file path, or stdin ('-').
    Add { payload: String },
    /// Deletes an ACME order.
    #[command(alias = "rm")]
    Delete { id: String },
    /// Triggers immediate renewal/issuance order for an ACME entry.
    Order { id: String },
    /// Shows CNAME delegation instructions and public DNS verification status for an ACME order.
    CnameInstructions { id: String },
}

#[derive(Debug, Subcommand)]
pub enum AclCommand {
    /// Lists all access lists.
    #[command(alias = "ls")]
    List,
    /// Shows details of an access list.
    Get { id: String },
    /// Adds an access list from inline JSON/YAML, a file path, or stdin ('-').
    Add { payload: String },
    /// Deletes an access list.
    #[command(alias = "rm")]
    Delete { id: String },
}

#[derive(Debug, Subcommand)]
pub enum ErrorPageThemeCommand {
    /// Lists all error page themes.
    #[command(alias = "ls")]
    List,
    /// Shows an error page theme by id.
    Get { id: String },
    /// Adds an error page theme from inline JSON/YAML, a file path, or stdin ('-').
    Add { payload: String },
    /// Updates an error page theme from inline JSON/YAML, a file path, or stdin ('-').
    Update { id: String, payload: String },
    /// Deletes an error page theme.
    #[command(alias = "rm")]
    Delete { id: String },
    /// Resets error page themes to standard default templates.
    ResetDefaults,
}

#[derive(Debug, Subcommand)]
pub enum DnsCommand {
    /// Lists all DNS provider credentials.
    #[command(alias = "ls")]
    List,
    /// Shows details of a DNS credential entry.
    Get { id: String },
    /// Adds a DNS credential entry from inline JSON/YAML, a file path, or stdin ('-').
    Add { payload: String },
    /// Deletes a DNS credential entry.
    #[command(alias = "rm")]
    Delete { id: String },
}

#[derive(Debug, Subcommand)]
pub enum AppCommand {
    /// Lists deployed applications.
    #[command(alias = "ls")]
    List,
    /// Shows configuration of an application.
    Get { id: String },
    /// Adds an application from inline JSON/YAML, a file path, or stdin ('-').
    Add { payload: String },
    /// Triggers a deployment for an application.
    Deploy { id: String },
    /// Rolls back an application to the previous deployment.
    Rollback { id: String },
    /// Starts an application container.
    Start { id: String },
    /// Stops an application container.
    Stop { id: String },
    /// Restarts an application container.
    Restart { id: String },
    /// Deletes an application.
    #[command(alias = "rm")]
    Delete { id: String },
    /// Displays application logs.
    Logs {
        id: String,
        #[arg(long, default_value = "100")]
        tail: usize,
    },
    /// Shows environment variables of an application.
    Env { id: String },
    /// Updates environment variables of an application (KEY=VALUE pairs).
    SetEnv {
        id: String,
        #[arg(required = true)]
        pairs: Vec<String>,
    },
    /// Shows container log file sizes of an application.
    LogSize { id: String },
    /// Truncates (vacuums) container log files of an application.
    VacuumLogs { id: String },
}

#[derive(Debug, Subcommand)]
pub enum DbCommand {
    /// Lists all managed databases.
    #[command(alias = "ls")]
    List,
    /// Shows configuration of a database.
    Get { id: String },
    /// Provisions a new database container.
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        engine: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        password: Option<String>,
    },
    /// Deletes a database container.
    #[command(alias = "rm")]
    Delete { id: String },
    /// Clones a database to a new instance.
    Clone {
        id: String,
        #[arg(long)]
        name: String,
    },
    /// Creates a read replica of a database.
    Replica {
        id: String,
        #[arg(long)]
        name: String,
    },
    /// Displays database container logs.
    Logs {
        id: String,
        #[arg(long, default_value = "100")]
        tail: usize,
    },
}

#[derive(Debug, Subcommand)]
pub enum BackupCommand {
    /// Lists backups for a database.
    #[command(alias = "ls")]
    List { database_id: String },
    /// Triggers an immediate backup for a database.
    Create { database_id: String },
    /// Restores a database from a backup.
    Restore {
        database_id: String,
        backup_id: String,
    },
    /// Deletes a backup snapshot.
    #[command(alias = "rm")]
    Delete {
        database_id: String,
        backup_id: String,
    },
    /// Shows backup configuration.
    Config,
}

#[derive(Debug, Subcommand)]
pub enum TargetCommand {
    /// Lists all multi-node agent targets.
    #[command(alias = "ls")]
    List,
    /// Shows details of a target node.
    Get { id: String },
    /// Enrolls a new target node.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        host: String,
        #[arg(long)]
        port: u16,
    },
    /// Deletes a target node.
    #[command(alias = "rm")]
    Delete { id: String },
    /// Drains a target node for maintenance.
    Drain { id: String },
    /// Undrains a target node, restoring it to traffic.
    Undrain { id: String },
    /// Generates a new target node enrollment token.
    Token,
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// Lists all projects and their environments.
    #[command(alias = "ls")]
    List,
    /// Shows details of a project.
    Get { id: String },
    /// Creates a new project.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Deletes a project.
    #[command(alias = "rm")]
    Delete { id: String },
    /// Synchronizes shared environment variables between project environments.
    SyncEnv {
        #[arg(long)]
        project: String,
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum StoreCommand {
    /// Lists available applications in the App Store catalog.
    #[command(alias = "ls")]
    List,
    /// Refreshes the local App Store catalog from the upstream repository.
    Refresh,
    /// Installs and routes an application from the App Store catalog.
    Install {
        #[arg(long)]
        app_id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        domain: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SecurityCommand {
    /// Lists all banned IP addresses.
    #[command(alias = "ls")]
    BannedIps,
    /// Unbans an IP address.
    Unban { ip: String },
    /// Lists all quarantined IP addresses.
    QuarantinedIps,
    /// Quarantines an IP address manually.
    Quarantine {
        /// Target IP address to quarantine.
        ip: String,
        /// Reason for quarantine.
        #[arg(long, short = 'r')]
        reason: Option<String>,
        /// Duration in seconds.
        #[arg(long, short = 'd')]
        duration_seconds: Option<u64>,
    },
    /// Releases an IP address from quarantine.
    Unquarantine {
        /// Target IP address to release.
        ip: String,
    },
    /// Shows auto-quarantine threat detection configuration.
    QuarantineConfig,
    /// Updates auto-quarantine threat detection configuration.
    SetQuarantineConfig {
        /// Whether threat detection and quarantine is enabled.
        #[arg(long)]
        enabled: Option<bool>,
        /// Maximum violations allowed in sliding window before ban.
        #[arg(long)]
        max_violations: Option<u32>,
        /// Sliding window in seconds.
        #[arg(long)]
        window_seconds: Option<u64>,
        /// Ban duration in seconds.
        #[arg(long)]
        ban_duration_seconds: Option<u64>,
    },
    /// Manage GeoIP MMDB database, MaxMind licensing, and automated updates.
    #[command(subcommand)]
    Geoip(GeoipCommand),
}

#[derive(Debug, Subcommand)]
pub enum GeoipCommand {
    /// Shows GeoIP MMDB database status, build timestamp, and auto-update configuration.
    #[command(alias = "info")]
    Status,
    /// Downloads and hot-reloads the latest GeoLite2-Country database from MaxMind.
    #[command(alias = "refresh")]
    Update,
    /// Shows the current GeoIP configuration.
    Config,
    /// Updates GeoIP configuration, MaxMind license key, and auto-update interval.
    SetConfig {
        /// MaxMind account ID.
        #[arg(long)]
        account_id: Option<String>,
        /// MaxMind license key for GeoLite2-Country downloads.
        #[arg(long)]
        license_key: Option<String>,
        /// Custom path to the MMDB file.
        #[arg(long)]
        database: Option<String>,
        /// Enable or disable automatic periodic background updates.
        #[arg(long)]
        auto_update: Option<bool>,
        /// Background update interval in days (1 to 90).
        #[arg(long)]
        interval_days: Option<u32>,
    },
}

#[derive(Debug, Subcommand)]
pub enum WebhookCommand {
    /// Lists configured outbound webhooks.
    #[command(alias = "ls")]
    List,
    /// Shows details of an outbound webhook.
    Get { id: String },
    /// Adds an outbound webhook from inline JSON/YAML, a file path, or stdin ('-').
    Add { payload: String },
    /// Deletes an outbound webhook.
    #[command(alias = "rm")]
    Delete { id: String },
    /// Sends a test ping to an outbound webhook.
    Test { id: String },
}

#[derive(Debug, Subcommand)]
pub enum CronCommand {
    /// Lists scheduled background commands for an application.
    #[command(alias = "ls")]
    List { app_id: String },
    /// Adds a scheduled command to an application.
    Add {
        app_id: String,
        #[arg(long)]
        cron: String,
        #[arg(long)]
        command: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Deletes a scheduled command.
    #[command(alias = "rm")]
    Delete { app_id: String, id: String },
    /// Runs a scheduled command immediately.
    Run { app_id: String, id: String },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Shows the current application settings.
    Get,
    /// Exports all configuration to a backup file (secrets masked, accounts excluded).
    Backup {
        /// File path to save the backup to, or standard output if omitted.
        #[arg(short = 'o', long = "output")]
        output: Option<String>,
    },
    /// Restores configuration from a backup file.
    Restore {
        /// File path of the backup to restore, or '-' to read from standard input.
        #[arg(short = 'f', long = "file")]
        file: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum PlatformCommand {
    /// Shows server status and runtime version.
    Status,
    /// Shows Prometheus system metrics and health.
    Metrics,
    /// Runs disk hygiene pruning (containers, unused images, build cache).
    Prune,
    /// Rotates the platform encryption key and re-encrypts all stored secrets.
    RotateKey,
    /// Truncates (vacuums) container log files across the platform.
    VacuumLogs {
        /// Optional minimum size threshold in MB for vacuuming.
        #[arg(long)]
        threshold_mb: Option<u64>,
    },
    /// Shows the host node resource threshold alert rules.
    NodeAlerts,
    /// Updates the host node resource threshold alert rules.
    SetNodeAlerts {
        /// Enable or disable node alerts (default: true).
        #[arg(long, default_value = "true")]
        enabled: bool,
        /// Minimum free disk space percentage threshold (e.g. 10).
        #[arg(long)]
        min_disk_percent: Option<u8>,
        /// Bandwidth spike threshold in bytes/sec (e.g. 104857600 for 100 MB/s).
        #[arg(long)]
        bandwidth_spike: Option<u64>,
        /// Maximum open file descriptor ratio percentage threshold (e.g. 80).
        #[arg(long)]
        max_fd_percent: Option<u8>,
        /// Maximum TCP TIME_WAIT socket count threshold (e.g. 10000).
        #[arg(long)]
        max_time_wait: Option<u32>,
    },
}

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
    if let Command::Update = args.command {
        return update(&client).await;
    }
    let server =
        origin(args.server.as_deref().context(
            "this command needs the server origin, for example http://127.0.0.1:46492",
        )?)?;
    if let Command::Login = args.command {
        return login(&client, &server).await;
    }
    run_with_token(&client, &server, args.token, args.command).await
}

async fn run_with_token(
    client: &reqwest::Client,
    server: &str,
    token: Option<String>,
    cmd: Command,
) -> anyhow::Result<()> {
    let access_token = access_token_of(client, server, token).await?;
    let api = api_client::ApiClient::new(client.clone(), server.to_string(), access_token);
    run_api(&api, cmd).await
}

async fn run_api(api: &api_client::ApiClient, cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Port(c) => run_port(api, c).await,
        Command::Proxy(c) => run_proxy(api, c).await,
        Command::Traffic(c) => run_traffic(api, c).await,
        Command::Cert(c) => run_cert(api, c).await,
        Command::Acme(c) => run_acme(api, c).await,
        Command::Acl(c) => run_acl(api, c).await,
        Command::ErrorPageTheme(c) => run_error_page_theme(api, c).await,
        Command::Dns(c) => run_dns(api, c).await,
        other => run_api_platform(api, other).await,
    }
}

async fn run_traffic(api: &api_client::ApiClient, cmd: TrafficCommand) -> anyhow::Result<()> {
    match cmd {
        TrafficCommand::Connections(conn_cmd) => run_traffic_connections(api, conn_cmd).await,
    }
}

async fn run_traffic_connections(
    api: &api_client::ApiClient,
    cmd: ConnectionsCommand,
) -> anyhow::Result<()> {
    match cmd {
        ConnectionsCommand::List { proxy } => api.list_active_connections(proxy.as_deref()).await,
        ConnectionsCommand::Close { id } => api.terminate_active_connection(&id).await,
    }
}

async fn run_api_platform(api: &api_client::ApiClient, cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::App(c) => run_app(api, c).await,
        Command::Db(c) => run_db(api, c).await,
        Command::Backup(c) => run_backup(api, c).await,
        Command::Cron(c) => run_cron(api, c).await,
        Command::Target(c) => run_target(api, c).await,
        other => run_api_system(api, other).await,
    }
}

async fn run_api_system(api: &api_client::ApiClient, cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Project(c) => run_project(api, c).await,
        Command::Store(c) => run_store(api, c).await,
        Command::Security(c) => run_security(api, c).await,
        Command::Webhook(c) => run_webhook(api, c).await,
        Command::Config(c) => run_config(api, c).await,
        Command::Platform(c) => run_platform(api, c).await,
        Command::Apply { file } => run_apply(api, &file).await,
        _ => unreachable!(),
    }
}

async fn run_platform(api: &api_client::ApiClient, cmd: PlatformCommand) -> anyhow::Result<()> {
    match cmd {
        PlatformCommand::RotateKey => api.rotate_platform_key().await,
        PlatformCommand::Status => api.platform_status().await,
        PlatformCommand::Metrics => api.platform_metrics().await,
        PlatformCommand::Prune => api.platform_prune().await,
        PlatformCommand::VacuumLogs { threshold_mb } => {
            api.vacuum_platform_logs(threshold_mb).await
        }
        PlatformCommand::NodeAlerts => api.get_node_alerts().await,
        PlatformCommand::SetNodeAlerts {
            enabled,
            min_disk_percent,
            bandwidth_spike,
            max_fd_percent,
            max_time_wait,
        } => {
            api.set_node_alerts(
                enabled,
                min_disk_percent,
                bandwidth_spike,
                max_fd_percent,
                max_time_wait,
            )
            .await
        }
    }
}

async fn run_port(api: &api_client::ApiClient, cmd: PortCommand) -> anyhow::Result<()> {
    match cmd {
        PortCommand::List => api.list_ports().await,
        PortCommand::Get { id } => api.get_port(&id).await,
        PortCommand::Add { payload } => api.add_port(&payload).await,
        PortCommand::Delete { id } => api.delete_port(&id).await,
        PortCommand::Reset { id } => api.reset_port(&id).await,
    }
}

async fn run_proxy(api: &api_client::ApiClient, cmd: ProxyCommand) -> anyhow::Result<()> {
    match cmd {
        ProxyCommand::List => api.list_proxies().await,
        ProxyCommand::Get { id } => api.get_proxy(&id).await,
        ProxyCommand::Add { payload } => api.add_proxy(&payload).await,
        ProxyCommand::Status { id } => api.proxy_status(&id).await,
        ProxyCommand::Delete { id } => api.delete_proxy(&id).await,
        ProxyCommand::PurgeCache { id, path } => api.purge_cache(&id, path.as_deref()).await,
        ProxyCommand::GrpcHealth {
            id,
            service,
            target_url,
        } => {
            api.check_proxy_grpc_health(&id, service.as_deref(), target_url.as_deref())
                .await
        }
        ProxyCommand::Dictionary(dict_cmd) => run_proxy_dictionary(api, dict_cmd).await,
        ProxyCommand::SecurityHeaders(sec_cmd) => run_proxy_security_headers(api, sec_cmd).await,
    }
}

async fn run_proxy_security_headers(
    api: &api_client::ApiClient,
    cmd: ProxySecurityHeadersCommand,
) -> anyhow::Result<()> {
    match cmd {
        ProxySecurityHeadersCommand::Audit { id } => api.audit_proxy_security_headers(&id).await,
        ProxySecurityHeadersCommand::Apply { id, profile } => {
            api.apply_proxy_security_profile(&id, &profile).await
        }
        ProxySecurityHeadersCommand::Profiles => api.list_security_profiles().await,
    }
}

async fn run_proxy_dictionary(
    api: &api_client::ApiClient,
    dict_cmd: ProxyDictionaryCommand,
) -> anyhow::Result<()> {
    match dict_cmd {
        ProxyDictionaryCommand::List { proxy_id } => api.list_proxy_dictionaries(&proxy_id).await,
        ProxyDictionaryCommand::Get { proxy_id, dict_id } => {
            api.get_proxy_dictionary(&proxy_id, &dict_id).await
        }
        ProxyDictionaryCommand::Add {
            proxy_id,
            name,
            match_pattern,
            path,
            data,
        } => {
            api.add_proxy_dictionary(&proxy_id, &name, &match_pattern, &path, &data)
                .await
        }
        ProxyDictionaryCommand::Delete { proxy_id, dict_id } => {
            api.delete_proxy_dictionary(&proxy_id, &dict_id).await
        }
    }
}

async fn run_cert(api: &api_client::ApiClient, cmd: CertCommand) -> anyhow::Result<()> {
    match cmd {
        CertCommand::List => api.list_certs().await,
        CertCommand::Get { id } => api.get_cert(&id).await,
        CertCommand::Upload { cert, key, kind } => {
            api.upload_cert(&cert, &key, kind.as_deref()).await
        }
        CertCommand::Delete { id } => api.delete_cert(&id).await,
        CertCommand::Coverage => api.cert_coverage().await,
    }
}

async fn run_acme(api: &api_client::ApiClient, cmd: AcmeCommand) -> anyhow::Result<()> {
    match cmd {
        AcmeCommand::List => api.list_acme().await,
        AcmeCommand::Get { id } => api.get_acme(&id).await,
        AcmeCommand::Add { payload } => api.add_acme(&payload).await,
        AcmeCommand::Delete { id } => api.delete_acme(&id).await,
        AcmeCommand::Order { id } => api.order_acme(&id).await,
        AcmeCommand::CnameInstructions { id } => api.get_acme_cname_instructions(&id).await,
    }
}

async fn run_acl(api: &api_client::ApiClient, cmd: AclCommand) -> anyhow::Result<()> {
    match cmd {
        AclCommand::List => api.list_acls().await,
        AclCommand::Get { id } => api.get_acl(&id).await,
        AclCommand::Add { payload } => api.add_acl(&payload).await,
        AclCommand::Delete { id } => api.delete_acl(&id).await,
    }
}

async fn run_error_page_theme(
    api: &api_client::ApiClient,
    cmd: ErrorPageThemeCommand,
) -> anyhow::Result<()> {
    match cmd {
        ErrorPageThemeCommand::List => api.list_error_page_themes().await,
        ErrorPageThemeCommand::Get { id } => api.get_error_page_theme(&id).await,
        ErrorPageThemeCommand::Add { payload } => api.add_error_page_theme(&payload).await,
        ErrorPageThemeCommand::Update { id, payload } => {
            api.update_error_page_theme(&id, &payload).await
        }
        ErrorPageThemeCommand::Delete { id } => api.delete_error_page_theme(&id).await,
        ErrorPageThemeCommand::ResetDefaults => api.reset_error_page_themes().await,
    }
}

async fn run_dns(api: &api_client::ApiClient, cmd: DnsCommand) -> anyhow::Result<()> {
    match cmd {
        DnsCommand::List => api.list_dns().await,
        DnsCommand::Get { id } => api.get_dns(&id).await,
        DnsCommand::Add { payload } => api.add_dns(&payload).await,
        DnsCommand::Delete { id } => api.delete_dns(&id).await,
    }
}

async fn run_app(api: &api_client::ApiClient, cmd: AppCommand) -> anyhow::Result<()> {
    match cmd {
        AppCommand::List => api.list_apps().await,
        AppCommand::Get { id } => api.get_app(&id).await,
        AppCommand::Add { payload } => api.add_app(&payload).await,
        AppCommand::Deploy { id } => api.deploy_app(&id).await,
        AppCommand::Rollback { id } => api.rollback_app(&id).await,
        AppCommand::Start { id } => api.start_app(&id).await,
        AppCommand::Stop { id } => api.stop_app(&id).await,
        AppCommand::Restart { id } => api.restart_app(&id).await,
        AppCommand::Delete { id } => api.delete_app(&id).await,
        AppCommand::Logs { id, tail } => api.app_logs(&id, tail).await,
        AppCommand::Env { id } => api.get_app_env(&id).await,
        AppCommand::SetEnv { id, pairs } => api.set_app_env(&id, &pairs).await,
        AppCommand::LogSize { id } => api.app_log_size(&id).await,
        AppCommand::VacuumLogs { id } => api.vacuum_app_logs(&id).await,
    }
}

async fn run_db(api: &api_client::ApiClient, cmd: DbCommand) -> anyhow::Result<()> {
    match cmd {
        DbCommand::List => api.list_dbs().await,
        DbCommand::Get { id } => api.get_db(&id).await,
        DbCommand::Create {
            name,
            engine,
            version,
            password,
        } => {
            api.create_db(&name, &engine, version.as_deref(), password.as_deref())
                .await
        }
        DbCommand::Delete { id } => api.delete_db(&id).await,
        DbCommand::Clone { id, name } => api.clone_db(&id, &name).await,
        DbCommand::Replica { id, name } => api.replica_db(&id, &name).await,
        DbCommand::Logs { id, tail } => api.db_logs(&id, tail).await,
    }
}

async fn run_backup(api: &api_client::ApiClient, cmd: BackupCommand) -> anyhow::Result<()> {
    match cmd {
        BackupCommand::List { database_id } => api.list_backups(&database_id).await,
        BackupCommand::Create { database_id } => api.create_backup(&database_id).await,
        BackupCommand::Restore {
            database_id,
            backup_id,
        } => api.restore_backup(&database_id, &backup_id).await,
        BackupCommand::Delete {
            database_id,
            backup_id,
        } => api.delete_backup(&database_id, &backup_id).await,
        BackupCommand::Config => api.backup_config().await,
    }
}

async fn run_target(api: &api_client::ApiClient, cmd: TargetCommand) -> anyhow::Result<()> {
    match cmd {
        TargetCommand::List => api.list_targets().await,
        TargetCommand::Get { id } => api.get_target(&id).await,
        TargetCommand::Add { name, host, port } => api.add_target(&name, &host, port).await,
        TargetCommand::Delete { id } => api.delete_target(&id).await,
        TargetCommand::Drain { id } => api.drain_target(&id).await,
        TargetCommand::Undrain { id } => api.undrain_target(&id).await,
        TargetCommand::Token => api.new_target_token().await,
    }
}

async fn run_project(api: &api_client::ApiClient, cmd: ProjectCommand) -> anyhow::Result<()> {
    match cmd {
        ProjectCommand::List => api.list_projects().await,
        ProjectCommand::Get { id } => api.get_project(&id).await,
        ProjectCommand::Add { name, description } => {
            api.create_project(&name, description.as_deref()).await
        }
        ProjectCommand::Delete { id } => api.delete_project(&id).await,
        ProjectCommand::SyncEnv { project, from, to } => {
            api.sync_project_env(&project, &from, &to).await
        }
    }
}

async fn run_store(api: &api_client::ApiClient, cmd: StoreCommand) -> anyhow::Result<()> {
    match cmd {
        StoreCommand::List => api.list_store().await,
        StoreCommand::Refresh => api.refresh_store().await,
        StoreCommand::Install {
            app_id,
            name,
            domain,
        } => api.install_store(&app_id, &name, domain.as_deref()).await,
    }
}

async fn run_security(api: &api_client::ApiClient, cmd: SecurityCommand) -> anyhow::Result<()> {
    match cmd {
        SecurityCommand::BannedIps => api.list_banned_ips().await,
        SecurityCommand::Unban { ip } => api.unban_ip(&ip).await,
        SecurityCommand::QuarantinedIps => api.list_quarantined_ips().await,
        SecurityCommand::Quarantine {
            ip,
            reason,
            duration_seconds,
        } => {
            api.quarantine_ip(&ip, reason.as_deref(), duration_seconds)
                .await
        }
        SecurityCommand::Unquarantine { ip } => api.unquarantine_ip(&ip).await,
        SecurityCommand::QuarantineConfig => api.get_quarantine_config().await,
        SecurityCommand::SetQuarantineConfig {
            enabled,
            max_violations,
            window_seconds,
            ban_duration_seconds,
        } => {
            api.set_quarantine_config(
                enabled,
                max_violations,
                window_seconds,
                ban_duration_seconds,
            )
            .await
        }
        SecurityCommand::Geoip(cmd) => run_geoip(api, cmd).await,
    }
}

async fn run_geoip(api: &api_client::ApiClient, cmd: GeoipCommand) -> anyhow::Result<()> {
    match cmd {
        GeoipCommand::Status => api.get_geoip_status().await,
        GeoipCommand::Update => api.update_geoip_database().await,
        GeoipCommand::Config => api.get_geoip_config().await,
        GeoipCommand::SetConfig {
            account_id,
            license_key,
            database,
            auto_update,
            interval_days,
        } => {
            api.set_geoip_config(
                account_id,
                license_key,
                database,
                auto_update,
                interval_days,
            )
            .await
        }
    }
}

async fn run_webhook(api: &api_client::ApiClient, cmd: WebhookCommand) -> anyhow::Result<()> {
    match cmd {
        WebhookCommand::List => api.list_webhooks().await,
        WebhookCommand::Get { id } => api.get_webhook(&id).await,
        WebhookCommand::Add { payload } => api.add_webhook(&payload).await,
        WebhookCommand::Delete { id } => api.delete_webhook(&id).await,
        WebhookCommand::Test { id } => api.test_webhook(&id).await,
    }
}

async fn run_cron(api: &api_client::ApiClient, cmd: CronCommand) -> anyhow::Result<()> {
    match cmd {
        CronCommand::List { app_id } => api.list_cron(&app_id).await,
        CronCommand::Add {
            app_id,
            cron,
            command,
            name,
        } => {
            api.add_cron(&app_id, &cron, &command, name.as_deref())
                .await
        }
        CronCommand::Delete { app_id, id } => api.delete_cron(&app_id, &id).await,
        CronCommand::Run { app_id, id } => api.run_cron(&app_id, &id).await,
    }
}

async fn run_config(api: &api_client::ApiClient, cmd: ConfigCommand) -> anyhow::Result<()> {
    match cmd {
        ConfigCommand::Get => api.get_config().await,
        ConfigCommand::Backup { output } => api.export_config_backup(output.as_deref()).await,
        ConfigCommand::Restore { file } => api.restore_config_backup(&file).await,
    }
}

async fn run_apply(api: &api_client::ApiClient, file: &str) -> anyhow::Result<()> {
    let content = if file == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .context("failed to read manifest from stdin")?;
        buf
    } else {
        std::fs::read_to_string(file)
            .with_context(|| format!("failed to read manifest file: {file}"))?
    };
    api.apply_manifest(&content).await
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

/// The GitHub repository that ships the releases of this client.
const RELEASES_API: &str = "https://api.github.com/repos/KilimcininKorOglu/r3v3rs3/releases/latest";

/// The release archive of this platform. The Linux archives hold the static musl build,
/// so they run on every distribution regardless of the installed libc.
fn asset_name() -> anyhow::Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("r3-x86_64-unknown-linux-musl.tar.gz"),
        ("linux", "aarch64") => Ok("r3-aarch64-unknown-linux-musl.tar.gz"),
        ("macos", "x86_64") => Ok("r3-x86_64-apple-darwin.tar.gz"),
        ("macos", "aarch64") => Ok("r3-aarch64-apple-darwin.tar.gz"),
        ("windows", "x86_64") => Ok("r3-x86_64-pc-windows-msvc.zip"),
        (os, arch) => bail!("no release archive exists for {os}/{arch}"),
    }
}

/// The name of the executable inside the archive of this platform.
fn binary_name() -> &'static str {
    if std::env::consts::OS == "windows" {
        "r3.exe"
    } else {
        "r3"
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
        .header(reqwest::header::USER_AGENT, "r3")
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
            // The sum file lists the names as `./r3v3rs3-cli-...`.
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
            println!("r3 {current} is the newest release.");
            return Ok(());
        }
        None => bail!("the release version {latest} is not three numbers"),
        Some(true) => {}
    }
    println!("Downloading r3 {latest}...");
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
    println!("r3 {latest} is installed. The next run uses it.");
    Ok(())
}

/// Downloads one release file as bytes.
async fn download(client: &reqwest::Client, url: &str) -> anyhow::Result<Vec<u8>> {
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, "r3")
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
    let temporary = dir.join(format!(".r3.new.{}", std::process::id()));
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
        let aside = dir.join("r3.old");
        let _ = std::fs::remove_file(&aside);
        std::fs::rename(&exe, &aside).context("the old binary could not be moved aside")?;
        std::fs::rename(&temporary, &exe).context("the running binary could not be replaced")?;
    }
    Ok(())
}

fn unpack_zip(archive: &[u8], name: &str) -> anyhow::Result<Vec<u8>> {
    let reader = std::io::Cursor::new(archive);
    let mut zip = zip::ZipArchive::new(reader).context("the archive is not readable")?;
    let mut file = zip
        .by_name(name)
        .context("the archive holds no r3 binary")?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes)
        .context("the archive could not be unpacked")?;
    Ok(bytes)
}

fn unpack_tar_gz(archive: &[u8], name: &str) -> anyhow::Result<Vec<u8>> {
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
    bail!("the archive holds no r3 binary")
}

/// Reads the executable out of the tar.gz or the zip archive of this platform.
fn unpack(archive: &[u8]) -> anyhow::Result<Vec<u8>> {
    let name = binary_name();
    if std::env::consts::OS == "windows" {
        unpack_zip(archive, name)
    } else {
        unpack_tar_gz(archive, name)
    }
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
        let sums = "abc123  ./r3-x86_64-unknown-linux-musl.tar.gz\ndef456  SHA256SUMS\n";
        assert_eq!(
            digest_of(sums, "r3-x86_64-unknown-linux-musl.tar.gz").unwrap(),
            "abc123"
        );
        assert!(digest_of(sums, "r3-aarch64-unknown-linux-musl.tar.gz").is_err());
    }
}
