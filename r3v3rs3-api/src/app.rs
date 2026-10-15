use crate::acme::webhook_url_allowed;
use crate::error::Error;
use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use std::{fmt, net::SocketAddr, path::PathBuf, time::Duration};
use url::Url;
use utoipa::ToSchema;

#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppConfig {
    #[serde(with = "humantime_serde", default = "default_background_task_interval")]
    #[schema(value_type = String, example = "1h")]
    pub background_task_interval: Duration,

    /// The longest time that the server waits for the open client connections when it stops.
    #[serde(
        with = "humantime_serde",
        default = "default_shutdown_graceful_timeout"
    )]
    #[schema(value_type = String, example = "30s")]
    pub shutdown_graceful_timeout: Duration,

    #[serde(default)]
    pub admin: AdminConfig,

    #[serde(default)]
    pub log: LogConfig,

    #[serde(default)]
    pub notifications: NotificationConfig,

    #[serde(default = "default_http_challenge_addr")]
    #[schema(value_type = String, example = "0.0.0.0:80")]
    pub http_challenge_addr: SocketAddr,

    /// The listening address of the ACME TLS-ALPN-01 challenges when no port uses its port.
    #[serde(default = "default_tls_alpn_challenge_addr")]
    #[schema(value_type = String, example = "0.0.0.0:443")]
    pub tls_alpn_challenge_addr: SocketAddr,

    /// DNS server that is asked whether the TXT records of a DNS-01 challenge are visible.
    /// The system resolver is used when it is not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "1.1.1.1:53")]
    pub dns_challenge_resolver: Option<SocketAddr>,

    /// DNS server that answers the SRV lookups of `http+srv` and `https+srv` server URLs.
    /// The system resolver is used when it is not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "127.0.0.1:8600")]
    pub upstream_dns_resolver: Option<SocketAddr>,

    /// The public address of the server that the Git providers send the webhook requests of
    /// the platform apps to, and that the OAuth clients of the MCP endpoint see as the
    /// issuer. Without it a Git connection installs no webhook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "https://deploy.example.com")]
    pub public_url: Option<crate::git_connection::PublicUrl>,

    #[serde(default)]
    pub discovery: crate::discovery::DiscoveryConfig,

    /// Only `config.toml` sets this section. An update through the admin API keeps it.
    #[serde(default)]
    pub acme_exec: AcmeExecConfig,

    /// The proxy worker processes. The default serves every port in this process.
    #[serde(default)]
    pub worker_mode: WorkerMode,

    /// Only `config.toml` sets this section. An update through the admin API keeps it.
    #[serde(default)]
    pub cluster: crate::cluster::ClusterConfig,

    /// Only `config.toml` sets this section. An update through the admin API keeps it.
    #[serde(default)]
    pub platform: crate::platform::PlatformConfig,

    /// Only `config.toml` sets this section. An update through the admin API keeps it.
    #[serde(default)]
    pub mcp: McpConfig,

    /// The default HTTP/2 flood limits of every server-side connection. A port can override
    /// them in its options.
    #[serde(default)]
    pub http2: Http2Limits,

    /// The default HTTP/3 stream limits of every QUIC endpoint. A port can override them in
    /// its options.
    #[serde(default)]
    pub http3: Http3Limits,

    /// The update check of the panel.
    #[serde(default)]
    pub upgrade: UpgradeConfig,
}

/// How often the server asks the release API whether a newer version exists. When one does,
/// the panel shows a banner on every page.
#[derive(Debug, DefaultFromSerde, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UpgradeConfig {
    #[serde(with = "humantime_serde", default = "default_upgrade_check_interval")]
    #[schema(value_type = String, example = "1h")]
    pub check_interval: Duration,
}

fn default_upgrade_check_interval() -> Duration {
    Duration::from_secs(60 * 60)
}

/// The HTTP/2 flood limits of a server-side connection.
#[derive(Debug, DefaultFromSerde, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Http2Limits {
    /// The largest HEADERS plus CONTINUATION size of one request, in bytes.
    #[serde(default = "default_max_header_list_size")]
    pub max_header_list_size: u32,
    /// The streams that a client can reset before a GOAWAY closes the connection.
    #[serde(default = "default_max_pending_accept_reset_streams")]
    pub max_pending_accept_reset_streams: usize,
}

impl Http2Limits {
    /// Checks the range of every limit.
    pub fn validate(&self) -> Result<(), Error> {
        const HEADER_LIST_MIN: u64 = 1024;
        const HEADER_LIST_MAX: u64 = 1024 * 1024;
        if !(HEADER_LIST_MIN..=HEADER_LIST_MAX).contains(&(self.max_header_list_size as u64)) {
            return Err(Error::InvalidHttpLimits {
                name: "max_header_list_size".into(),
                min: HEADER_LIST_MIN,
                max: HEADER_LIST_MAX,
            });
        }
        const RESET_STREAMS_MIN: u64 = 1;
        const RESET_STREAMS_MAX: u64 = 65536;
        if !(RESET_STREAMS_MIN..=RESET_STREAMS_MAX)
            .contains(&(self.max_pending_accept_reset_streams as u64))
        {
            return Err(Error::InvalidHttpLimits {
                name: "max_pending_accept_reset_streams".into(),
                min: RESET_STREAMS_MIN,
                max: RESET_STREAMS_MAX,
            });
        }
        Ok(())
    }
}

fn default_max_header_list_size() -> u32 {
    65536
}

fn default_max_pending_accept_reset_streams() -> usize {
    128
}

/// The HTTP/3 stream limits of a QUIC endpoint.
#[derive(Debug, DefaultFromSerde, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Http3Limits {
    /// The request streams that one QUIC connection can open.
    #[serde(default = "default_max_concurrent_bidi_streams")]
    pub max_concurrent_bidi_streams: u32,
    /// The control streams that one QUIC connection can open.
    #[serde(default = "default_max_concurrent_uni_streams")]
    pub max_concurrent_uni_streams: u32,
}

impl Http3Limits {
    /// Checks the range of every limit.
    pub fn validate(&self) -> Result<(), Error> {
        for (name, value) in [
            (
                "max_concurrent_bidi_streams",
                self.max_concurrent_bidi_streams,
            ),
            (
                "max_concurrent_uni_streams",
                self.max_concurrent_uni_streams,
            ),
        ] {
            if !(1..=100000).contains(&value) {
                return Err(Error::InvalidHttpLimits {
                    name: name.into(),
                    min: 1,
                    max: 100000,
                });
            }
        }
        Ok(())
    }
}

fn default_max_concurrent_bidi_streams() -> u32 {
    100
}

fn default_max_concurrent_uni_streams() -> u32 {
    10
}

/// The programs that the exec DNS provider can run.
#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AcmeExecConfig {
    /// Absolute paths of the programs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<String>, example = json!(["/usr/local/bin/r3v3rs3-dns-hook"]))]
    pub programs: Vec<PathBuf>,

    /// Longest run time of one program call.
    #[serde(with = "humantime_serde", default = "default_acme_exec_timeout")]
    #[schema(value_type = String, example = "30s")]
    pub timeout: Duration,
}

fn default_acme_exec_timeout() -> Duration {
    Duration::from_secs(30)
}

impl AppConfig {
    /// A copy without secrets, which the admin API returns.
    pub fn masked(&self) -> Self {
        Self {
            notifications: self.notifications.masked(),
            discovery: self.discovery.masked(),
            cluster: self.cluster.masked(),
            ..self.clone()
        }
    }

    /// Takes the secrets that an update does not set from the current settings.
    pub fn keep_secrets(&mut self, current: &Self) {
        self.notifications.keep_secrets(&current.notifications);
        self.discovery.keep_secrets(&current.discovery);
    }

    /// Takes the settings that only `config.toml` sets from the current settings. The cluster
    /// and the worker mode change the process itself, so a restart applies them.
    pub fn keep_file_only(&mut self, current: &Self) {
        self.cluster.clone_from(&current.cluster);
        self.worker_mode.clone_from(&current.worker_mode);
    }
}

fn default_background_task_interval() -> Duration {
    Duration::from_secs(60 * 60)
}

fn default_shutdown_graceful_timeout() -> Duration {
    Duration::from_secs(30)
}

/// The Model Context Protocol server of the admin API and the clients that may sign in to it.
#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct McpConfig {
    /// The MCP endpoint answers only when this is on.
    #[serde(default)]
    pub enabled: bool,
    /// The clients that may run the authorization code flow. A client that is not in this
    /// list cannot sign in, unless it registers itself dynamically.
    #[serde(default)]
    pub clients: Vec<McpClientConfig>,
}

/// One pre-registered MCP client.
#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct McpClientConfig {
    /// The `client_id` that the client sends.
    pub id: String,
    /// The name that the consent form shows.
    pub name: String,
    /// The redirect URIs that the authorization request may use. `http` is allowed on the
    /// loopback addresses only, per RFC 8252.
    pub redirect_uris: Vec<String>,
}

impl McpClientConfig {
    /// The registered client with this id.
    pub fn find<'a>(clients: &'a [McpClientConfig], id: &str) -> Option<&'a McpClientConfig> {
        clients.iter().find(|client| client.id == id)
    }

    /// Whether the redirect URI belongs to this client. A loopback redirect may use any port,
    /// per RFC 8252.
    pub fn allows_redirect(&self, redirect_uri: &str) -> bool {
        self.redirect_uris.iter().any(|registered| {
            registered == redirect_uri || is_loopback_redirect(registered, redirect_uri)
        })
    }
}

/// Matches a registered loopback redirect against one with another port.
fn is_loopback_redirect(registered: &str, redirect_uri: &str) -> bool {
    let (Ok(registered), Ok(uri)) = (Url::parse(registered), Url::parse(redirect_uri)) else {
        return false;
    };
    let loopback = |host: &str| matches!(host, "127.0.0.1" | "[::1]" | "localhost");
    registered.scheme() == "http"
        && uri.scheme() == "http"
        && registered.host_str().is_some_and(loopback)
        && uri.host_str().is_some_and(loopback)
        && registered.path() == uri.path()
        && registered.query() == uri.query()
}

/// How the proxy ports are served.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerMode {
    /// This process serves every port.
    #[default]
    Off,
    /// The ports are served by worker processes that this process supervises.
    Workers,
}

/// What one worker process serves right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkerStatusEntry {
    pub pid: u32,
    /// Seconds since the worker process started.
    pub uptime_secs: u64,
    pub open_connections: u64,
}

/// The workers of this master and what each one reports. A worker that missed its
/// heartbeat answer stays listed as `unreachable` instead of disappearing.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkerStatusReport {
    pub mode: WorkerMode,
    pub workers: Vec<WorkerEntry>,
}

/// One worker slot. An unreachable worker holds only its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkerEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkerStatusEntry>,
}

fn default_http_challenge_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 80))
}

fn default_tls_alpn_challenge_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 443))
}

#[derive(Clone, Serialize, ToSchema)]
pub struct AppInfo {
    #[schema(example = "0.0.0")]
    pub version: &'static str,
    #[schema(example = "x86_64-unknown-linux-gnu")]
    pub target: &'static str,
    #[schema(example = "debug")]
    pub profile: &'static str,
    #[schema(example = json!([]))]
    pub features: &'static [&'static str],
    #[schema(example = "rustc 1.69.0 (84c898d65 2023-04-16)")]
    pub rustc: &'static str,
    #[schema(value_type = String, example = "/home/r3v3rs3/.config/r3v3rs3")]
    pub config_path: PathBuf,
    #[schema(value_type = String, example = "/home/r3v3rs3/.config/r3v3rs3")]
    pub log_path: PathBuf,
    /// Whether the deployment platform serves the admin API of this server.
    #[serde(default)]
    pub platform_enabled: bool,
    /// The tag of the newest release, when it is newer than this build. `None` means that
    /// this build is current or that no check has answered yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_available: Option<String>,
}

/// The answer of the server upgrade endpoint: the version that the next process starts.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpgradeStarted {
    #[schema(example = "v1.6.12")]
    pub tag: String,
}

/// The step of the server upgrade that runs at the moment. `None` means that no upgrade runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeStage {
    /// The release information is fetched from the release API.
    CheckingRelease,
    /// The release archive is downloaded.
    Downloading,
    /// The archive is verified and installed.
    Installing,
    /// The new process starts and takes the listeners over.
    HandingOver,
    /// The upgrade finished; the new version serves.
    Done,
}

/// The answer of the upgrade status endpoint.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpgradeStageResponse {
    pub stage: Option<UpgradeStage>,
}

/// The query parameters the update banner adds to the settings page link, so the page
/// scrolls to the upgrade card.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpgradeQuery {
    /// The page section the link points at.
    pub section: String,
}

/// The IANA time zones that the panel can render the stored times in. The list is fixed, so a
/// typo in `config.toml` or in the settings page cannot select a zone that the browser cannot
/// resolve.
pub const DISPLAY_TIMEZONES: [&str; 17] = [
    "UTC",
    "Europe/Istanbul",
    "Europe/London",
    "Europe/Berlin",
    "Europe/Paris",
    "Europe/Amsterdam",
    "Europe/Moscow",
    "America/New_York",
    "America/Chicago",
    "America/Los_Angeles",
    "America/Sao_Paulo",
    "Asia/Dubai",
    "Asia/Kolkata",
    "Asia/Singapore",
    "Asia/Hong_Kong",
    "Asia/Tokyo",
    "Australia/Sydney",
];

#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AdminConfig {
    #[serde(with = "humantime_serde", default = "default_admin_session_expiry")]
    #[schema(value_type = String, example = "1d")]
    pub session_expiry: Duration,

    #[serde(default = "default_max_attempts")]
    pub max_login_attempts: u32,

    #[serde(with = "humantime_serde", default = "default_login_attempts_reset")]
    #[schema(value_type = String, example = "15m")]
    pub login_attempts_reset: Duration,

    /// The IANA time zone that the panel renders the stored times in. The stored times stay
    /// UTC; this only changes how the panel shows them.
    #[serde(default = "default_display_timezone")]
    pub display_timezone: String,
}

impl AdminConfig {
    pub fn validate(&self) -> Result<(), Error> {
        if !DISPLAY_TIMEZONES.contains(&self.display_timezone.as_str()) {
            return Err(Error::InvalidDisplayTimezone {
                timezone: self.display_timezone.clone(),
            });
        }
        Ok(())
    }
}

fn default_display_timezone() -> String {
    "UTC".to_owned()
}

fn default_admin_session_expiry() -> Duration {
    Duration::from_secs(60 * 60)
}

fn default_max_attempts() -> u32 {
    10
}

fn default_login_attempts_reset() -> Duration {
    Duration::from_secs(60 * 15)
}

#[derive(Debug, DefaultFromSerde, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LogConfig {
    #[serde(with = "humantime_serde", default = "default_database_log_retention")]
    #[schema(value_type = String, example = "3months")]
    pub database_log_retention: Duration,

    /// How long the audit log keeps an entry.
    #[serde(with = "humantime_serde", default = "default_audit_log_retention")]
    #[schema(value_type = String, example = "1year")]
    pub audit_log_retention: Duration,
}

#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NotificationConfig {
    /// The certificate list marks a certificate that expires within this time, and the webhook
    /// gets a notification for it.
    #[serde(with = "humantime_serde", default = "default_cert_expiry_warning")]
    #[schema(value_type = String, example = "14days")]
    pub cert_expiry_warning: Duration,

    /// The webhook that gets the certificate notifications. Without it no notification is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookConfig>,
}

impl NotificationConfig {
    /// A copy without the webhook token. `token_set` tells whether a token is set.
    pub fn masked(&self) -> Self {
        let mut masked = self.clone();
        if let Some(webhook) = &mut masked.webhook {
            webhook.token_set = webhook.token.take().is_some();
        }
        masked
    }

    /// Takes the webhook token that an update does not set from the current settings. An empty
    /// token removes the token.
    pub fn keep_secrets(&mut self, current: &Self) {
        if let Some(webhook) = &mut self.webhook {
            webhook.token_set = false;
            let token = current
                .webhook
                .as_ref()
                .and_then(|saved| saved.token.clone());
            crate::discovery::keep_secret(&mut webhook.token, &token);
        }
    }

    pub fn validate(&self) -> Result<(), Error> {
        match &self.webhook {
            Some(webhook) if !webhook_url_allowed(&webhook.url) => {
                Err(Error::NotificationWebhookUrlInvalid {
                    url: webhook.url.clone(),
                })
            }
            Some(webhook) if webhook.timeout.is_zero() => Err(Error::InvalidTimeout),
            _ => Ok(()),
        }
    }
}

/// A service of the operator that gets a JSON `POST` request for each notification.
#[derive(DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WebhookConfig {
    /// An https URL, or an http URL on a loopback address.
    #[serde(default)]
    #[schema(example = "https://hooks.example.com/r3v3rs3")]
    pub url: String,
    /// The bearer token of the `Authorization` header. The admin API does not return it. An
    /// update without a token keeps the current token, and an empty token removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(write_only)]
    pub token: Option<String>,
    /// Whether a token is set.
    #[serde(default, skip_serializing_if = "is_false")]
    #[schema(read_only)]
    pub token_set: bool,
    /// The longest time of one request.
    #[serde(with = "humantime_serde", default = "default_webhook_timeout")]
    #[schema(value_type = String, example = "10s")]
    pub timeout: Duration,
}

impl fmt::Debug for WebhookConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebhookConfig")
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_cert_expiry_warning() -> Duration {
    Duration::from_secs(14 * 24 * 60 * 60)
}

fn default_webhook_timeout() -> Duration {
    Duration::from_secs(10)
}

/// Three months in the duration format: 30.44 days each.
fn default_database_log_retention() -> Duration {
    Duration::from_secs(3 * 2_630_016)
}

/// One year in the duration format: 365.25 days.
fn default_audit_log_retention() -> Duration {
    Duration::from_secs(31_557_600)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webhook(url: &str, token: Option<&str>) -> NotificationConfig {
        NotificationConfig {
            webhook: Some(WebhookConfig {
                url: url.into(),
                token: token.map(str::to_string),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn the_admin_api_hides_the_webhook_token_and_an_update_keeps_it() {
        let saved = webhook("https://hooks.example.com/", Some("secret-token"));
        assert!(!format!("{saved:?}").contains("secret-token"));
        let masked = saved.masked();
        let hidden = masked.webhook.clone().unwrap();
        assert_eq!((hidden.token, hidden.token_set), (None, true));

        let mut update = masked;
        update.keep_secrets(&saved);
        assert_eq!(update, saved);

        let mut cleared = webhook("https://hooks.example.com/", Some(""));
        cleared.keep_secrets(&saved);
        assert_eq!(cleared.webhook.unwrap().token, None);
    }

    #[test]
    fn a_webhook_needs_https_unless_it_is_a_loopback_address() {
        assert!(NotificationConfig::default().validate().is_ok());
        assert!(
            webhook("https://hooks.example.com/", None)
                .validate()
                .is_ok()
        );
        assert!(webhook("http://127.0.0.1:9000/", None).validate().is_ok());
        assert!(matches!(
            webhook("http://hooks.example.com/", None).validate(),
            Err(Error::NotificationWebhookUrlInvalid { .. })
        ));
        let mut no_timeout = webhook("https://hooks.example.com/", None);
        if let Some(webhook) = &mut no_timeout.webhook {
            webhook.timeout = Duration::ZERO;
        }
        assert!(matches!(no_timeout.validate(), Err(Error::InvalidTimeout)));
    }

    #[test]
    fn the_default_log_retentions_keep_their_documented_text() {
        let value = serde_json::to_value(LogConfig::default()).unwrap();
        assert_eq!(value["database_log_retention"], "3months");
        assert_eq!(value["audit_log_retention"], "1year");
    }
}
