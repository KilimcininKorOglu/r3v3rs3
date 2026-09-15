use crate::acme::webhook_url_allowed;
use crate::error::Error;
use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use std::{fmt, net::SocketAddr, path::PathBuf, time::Duration};
use utoipa::ToSchema;

#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppConfig {
    #[serde(with = "humantime_serde", default = "default_background_task_interval")]
    #[schema(value_type = String, example = "1h")]
    pub background_task_interval: Duration,

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

    #[serde(default)]
    pub discovery: crate::discovery::DiscoveryConfig,

    /// Only `config.toml` sets this section. An update through the admin API keeps it.
    #[serde(default)]
    pub acme_exec: AcmeExecConfig,

    /// Only `config.toml` sets this section. An update through the admin API keeps it.
    #[serde(default)]
    pub cluster: crate::cluster::ClusterConfig,
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

    /// Takes the settings that only `config.toml` sets from the current settings.
    pub fn keep_file_only(&mut self, current: &Self) {
        self.acme_exec.clone_from(&current.acme_exec);
        self.cluster.clone_from(&current.cluster);
    }
}

fn default_background_task_interval() -> Duration {
    Duration::from_secs(60 * 60)
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
    #[schema(example = "aarch64-apple-darwin")]
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
}

#[derive(Debug, DefaultFromSerde, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AdminConfig {
    #[serde(with = "humantime_serde", default = "default_admin_session_expiry")]
    #[schema(value_type = String, example = "1d")]
    pub session_expiry: Duration,

    #[serde(default = "default_max_attempts")]
    pub max_login_attempts: u32,

    #[serde(with = "humantime_serde", default = "default_login_attempts_reset")]
    #[schema(value_type = String, example = "15m")]
    pub login_attempts_reset: Duration,
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
        assert!(webhook("https://hooks.example.com/", None)
            .validate()
            .is_ok());
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
