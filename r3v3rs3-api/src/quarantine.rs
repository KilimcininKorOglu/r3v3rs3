//! Models and validation for automatic threat detection and IP quarantine.

use crate::error::Error;
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use utoipa::ToSchema;

/// Threat intelligence and behavioral auto-quarantine configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AutoQuarantineConfig {
    /// Whether automatic traffic threat detection and IP quarantine is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Maximum bad requests or scan violations within the sliding window before an IP is quarantined.
    #[serde(default = "default_max_violations")]
    pub max_violations: u32,
    /// Sliding inspection window in seconds (e.g. 60).
    #[serde(default = "default_window_seconds")]
    pub window_seconds: u64,
    /// Duration in seconds to quarantine the offending IP (e.g. 3600 for 1 hour).
    #[serde(default = "default_ban_duration_seconds")]
    pub ban_duration_seconds: u64,
    /// HTTP status codes considered violations when returned to the client (e.g. 404, 403, 401).
    #[serde(default = "default_block_status_codes")]
    pub block_status_codes: Vec<u16>,
    /// Scanner paths matched on whole path segments; each match is one violation
    /// (e.g. "/.env", "/.git").
    #[serde(default = "default_scanner_patterns")]
    pub scanner_patterns: Vec<String>,
    /// Client addresses and networks that are never quarantined, for example the network of
    /// the administrators.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = [String], example = json!(["198.51.100.0/24"]))]
    pub allowed_ips: Vec<IpNet>,
}

fn default_true() -> bool {
    true
}

fn default_max_violations() -> u32 {
    5
}

fn default_window_seconds() -> u64 {
    60
}

fn default_ban_duration_seconds() -> u64 {
    3600
}

fn default_block_status_codes() -> Vec<u16> {
    vec![404, 403]
}

fn default_scanner_patterns() -> Vec<String> {
    vec![
        "/.env".to_string(),
        "/.git".to_string(),
        "/.aws".to_string(),
        "/wp-login".to_string(),
        "/wp-admin".to_string(),
        "/xmlrpc.php".to_string(),
        "/phpmyadmin".to_string(),
        "/config.json".to_string(),
    ]
}

impl Default for AutoQuarantineConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            max_violations: default_max_violations(),
            window_seconds: default_window_seconds(),
            ban_duration_seconds: default_ban_duration_seconds(),
            block_status_codes: default_block_status_codes(),
            scanner_patterns: default_scanner_patterns(),
            allowed_ips: Vec::new(),
        }
    }
}

impl AutoQuarantineConfig {
    /// Whether the address is in `allowed_ips`, so it is never quarantined.
    pub fn allows(&self, ip: &IpAddr) -> bool {
        self.allowed_ips.iter().any(|net| net.contains(ip))
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.max_violations == 0 {
            return Err(Error::InvalidAutoQuarantineConfig {
                reason: "max_violations must be greater than 0".to_string(),
            });
        }
        if self.window_seconds == 0 {
            return Err(Error::InvalidAutoQuarantineConfig {
                reason: "window_seconds must be greater than 0".to_string(),
            });
        }
        if self.ban_duration_seconds == 0 {
            return Err(Error::InvalidAutoQuarantineConfig {
                reason: "ban_duration_seconds must be greater than 0".to_string(),
            });
        }
        Ok(())
    }
}

/// A quarantined IP address entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuarantinedIpEntry {
    #[schema(value_type = String, example = "203.0.113.195")]
    pub ip: IpAddr,
    /// Reason why the IP was quarantined (e.g. "Scanner pattern: /.env" or "Repeated 404 errors").
    pub reason: String,
    /// Total violations recorded before or during quarantine.
    pub violations: u32,
    /// Unix timestamp in milliseconds when the IP was quarantined.
    pub banned_at: u64,
    /// Unix timestamp in milliseconds when the quarantine expires.
    pub banned_until: u64,
    /// Remaining seconds until quarantine expiration.
    pub remaining_seconds: u64,
}

/// Request to manually quarantine an IP address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuarantineIpRequest {
    #[schema(value_type = String, example = "203.0.113.195")]
    pub ip: IpAddr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = AutoQuarantineConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_violations, 5);
        assert_eq!(config.window_seconds, 60);
        assert_eq!(config.ban_duration_seconds, 3600);
        assert!(config.scanner_patterns.contains(&"/.env".to_string()));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn invalid_config_rejected() {
        let config = AutoQuarantineConfig {
            max_violations: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let config = AutoQuarantineConfig {
            window_seconds: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let config = AutoQuarantineConfig {
            ban_duration_seconds: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn an_allowed_network_covers_its_addresses_only() {
        let config: AutoQuarantineConfig =
            serde_json::from_str(r#"{"allowed_ips": ["198.51.100.0/24", "2001:db8::1/128"]}"#)
                .unwrap();
        assert!(config.allows(&"198.51.100.7".parse().unwrap()));
        assert!(config.allows(&"2001:db8::1".parse().unwrap()));
        assert!(!config.allows(&"198.51.101.7".parse().unwrap()));
        assert!(!AutoQuarantineConfig::default().allows(&"198.51.100.7".parse().unwrap()));
    }

    #[test]
    fn quarantine_entry_roundtrip() {
        let entry = QuarantinedIpEntry {
            ip: "198.51.100.22".parse().unwrap(),
            reason: "Scanner pattern: /.env".to_string(),
            violations: 1,
            banned_at: 1000,
            banned_until: 4600000,
            remaining_seconds: 3600,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: QuarantinedIpEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, parsed);
    }
}
