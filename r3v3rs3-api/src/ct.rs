//! Certificate Transparency (CT) log monitor and rogue certificate detection models.

use serde_derive::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// The alert/investigation status of a Certificate Transparency log entry.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CtAlertStatus {
    #[default]
    Investigating,
    Dismissed,
    ConfirmedRogue,
}

/// A Certificate Transparency log record discovered for a monitored domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CtLogEntry {
    /// Unique identifier (e.g. crt.sh ID or fingerprint).
    pub id: String,
    /// The domain queried or matched.
    pub domain: String,
    /// Distinguished name of the issuing CA.
    pub issuer_name: String,
    /// Common name (CN) on the certificate.
    pub common_name: String,
    /// Subject Alternative Names (SANs) on the certificate.
    #[serde(default)]
    pub san: Vec<String>,
    /// Serial number of the certificate in hex format.
    pub serial_number: String,
    /// Unix timestamp when logged into CT (if known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logged_at: Option<i64>,
    /// Unix timestamp when certificate validity begins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<i64>,
    /// Unix timestamp when certificate validity expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<i64>,
    /// Whether this entry is flagged as suspicious or rogue.
    pub is_rogue: bool,
    /// Reason explaining why the certificate was flagged as rogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rogue_reason: Option<String>,
    /// Current alert resolution status.
    pub status: CtAlertStatus,
}

/// Configuration for the background Certificate Transparency log monitor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CtMonitorConfig {
    /// Whether the background CT log monitor is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Periodic check interval in seconds (default: 21600 = 6 hours, min: 300).
    #[serde(default = "default_interval")]
    pub check_interval_secs: u64,
    /// Explicit list of domains to monitor in CT logs.
    #[serde(default)]
    pub monitored_domains: Vec<String>,
    /// Automatically discover domains from server certificates and ACME orders.
    #[serde(default = "default_true")]
    pub auto_discover_domains: bool,
    /// Allowed or trusted CA issuer substrings (e.g. "Let's Encrypt", "DigiCert", "Google Trust Services").
    #[serde(default = "default_allowed_issuers")]
    pub allowed_issuers: Vec<String>,
    /// Whether to flag certificates from unknown/unauthorized CAs as rogue.
    #[serde(default = "default_true")]
    pub alert_on_unknown_issuer: bool,
    /// Custom CT API template (e.g. "https://crt.sh/?q={domain}&output=json").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ct_api_url: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_interval() -> u64 {
    21600
}

/// Predefined list of commonly trusted public CAs.
pub fn default_allowed_issuers() -> Vec<String> {
    vec![
        "Let's Encrypt".to_string(),
        "Google Trust Services".to_string(),
        "DigiCert".to_string(),
        "Sectigo".to_string(),
        "ZeroSSL".to_string(),
        "Cloudflare".to_string(),
    ]
}

impl Default for CtMonitorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_secs: default_interval(),
            monitored_domains: Vec::new(),
            auto_discover_domains: true,
            allowed_issuers: default_allowed_issuers(),
            alert_on_unknown_issuer: true,
            ct_api_url: None,
        }
    }
}

/// Overall runtime status of the Certificate Transparency monitor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CtMonitorStatus {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_check_at: Option<i64>,
    pub monitored_domains: Vec<String>,
    pub total_entries_count: usize,
    pub active_rogue_alerts_count: usize,
    pub last_check_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Request body for updating CT monitor configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UpdateCtConfigRequest {
    pub config: CtMonitorConfig,
}

/// Query parameters for triggering an immediate CT log check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
pub struct CheckCtQuery {
    pub domain: Option<String>,
}

/// Request body for dismissing or resolving a rogue certificate alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DismissCtAlertRequest {
    pub id: String,
    pub status: Option<CtAlertStatus>,
}

/// Query parameters for listing CT log entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
pub struct CtEntriesQuery {
    pub domain: Option<String>,
    pub rogue_only: Option<bool>,
    pub limit: Option<usize>,
}

/// Checks whether an issuer matches any of the allowed CA issuer substrings.
pub fn is_issuer_allowed(issuer: &str, allowed_issuers: &[String]) -> bool {
    let issuer_lower = issuer.to_lowercase();
    allowed_issuers.iter().any(|allowed| {
        let pattern = allowed.trim().to_lowercase();
        !pattern.is_empty() && issuer_lower.contains(&pattern)
    })
}

/// Parses an ISO 8601 or similar UTC timestamp string into Unix timestamp seconds.
pub fn parse_ct_timestamp(s: &str) -> Option<i64> {
    let clean = s.trim();
    if clean.is_empty() {
        return None;
    }
    // Formats like "2024-05-10T12:00:00" or "2024-05-10 12:00:00"
    let parts: Vec<&str> = clean.split(['T', ' ']).collect();
    if parts.is_empty() {
        return None;
    }
    let date_parts: Vec<i64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    if date_parts.len() != 3 {
        return None;
    }
    let year = date_parts[0];
    let month = date_parts[1];
    let day = date_parts[2];

    let mut hour = 0i64;
    let mut min = 0i64;
    let mut sec = 0i64;
    if parts.len() > 1 {
        let time_parts: Vec<i64> = parts[1].split(':').filter_map(|p| p.parse().ok()).collect();
        if !time_parts.is_empty() {
            hour = time_parts[0];
        }
        if time_parts.len() > 1 {
            min = time_parts[1];
        }
        if time_parts.len() > 2 {
            sec = time_parts[2];
        }
    }
    calculate_unix_time(year, month, day, hour, min, sec)
}

fn calculate_unix_time(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    min: i64,
    sec: i64,
) -> Option<i64> {
    if !(1970..=2100).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days since unix epoch (1970-01-01)
    let days_before_month = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut total_days = (year - 1970) * 365 + (year - 1969) / 4;
    total_days += days_before_month[(month - 1) as usize];
    if month > 2 && ((year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)) {
        total_days += 1;
    }
    total_days += day - 1;
    let total_secs = total_days * 86400 + hour * 3600 + min * 60 + sec;
    Some(total_secs)
}

/// Parses a crt.sh JSON item into a `CtLogEntry`.
pub fn parse_crt_sh_entry(
    value: &serde_json::Value,
    domain: &str,
    allowed_issuers: &[String],
    alert_on_unknown: bool,
) -> Option<CtLogEntry> {
    let id_val = value.get("id")?;
    let id = match id_val {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        _ => return None,
    };
    let issuer_name = value
        .get("issuer_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let common_name = value
        .get("common_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name_value = value
        .get("name_value")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let serial_number = value
        .get("serial_number")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let san: Vec<String> = name_value
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let logged_at = value
        .get("entry_timestamp")
        .and_then(|v| v.as_str())
        .and_then(parse_ct_timestamp);
    let not_before = value
        .get("not_before")
        .and_then(|v| v.as_str())
        .and_then(parse_ct_timestamp);
    let not_after = value
        .get("not_after")
        .and_then(|v| v.as_str())
        .and_then(parse_ct_timestamp);

    let allowed = is_issuer_allowed(&issuer_name, allowed_issuers);
    let is_rogue = alert_on_unknown && !allowed && !issuer_name.is_empty();
    let rogue_reason = if is_rogue {
        Some(format!(
            "Untrusted or unauthorized CA issuer: {issuer_name}"
        ))
    } else {
        None
    };

    Some(CtLogEntry {
        id,
        domain: domain.to_string(),
        issuer_name,
        common_name,
        san,
        serial_number,
        logged_at,
        not_before,
        not_after,
        is_rogue,
        rogue_reason,
        status: CtAlertStatus::Investigating,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_issuer_allowed() {
        let allowed = vec![
            "Let's Encrypt".to_string(),
            "Google Trust Services".to_string(),
        ];
        assert!(is_issuer_allowed("C=US, O=Let's Encrypt, CN=R3", &allowed));
        assert!(is_issuer_allowed("O=google trust services", &allowed));
        assert!(!is_issuer_allowed("C=CN, O=Suspicious Authority", &allowed));
    }

    #[test]
    fn test_parse_ct_timestamp() {
        let ts = parse_ct_timestamp("2024-01-01T00:00:00").unwrap();
        assert_eq!(ts, 1704067200);
    }

    #[test]
    fn test_parse_crt_sh_entry() {
        let json = serde_json::json!({
            "id": 123456,
            "issuer_name": "C=US, O=Let's Encrypt, CN=R3",
            "common_name": "example.com",
            "name_value": "example.com\nwww.example.com",
            "serial_number": "03deadbeef",
            "entry_timestamp": "2024-01-01T00:00:00",
            "not_before": "2024-01-01T00:00:00",
            "not_after": "2024-04-01T00:00:00"
        });
        let allowed = default_allowed_issuers();
        let entry = parse_crt_sh_entry(&json, "example.com", &allowed, true).unwrap();
        assert_eq!(entry.id, "123456");
        assert_eq!(entry.domain, "example.com");
        assert_eq!(entry.san.len(), 2);
        assert!(!entry.is_rogue);

        let rogue_json = serde_json::json!({
            "id": 999999,
            "issuer_name": "C=RU, O=Rogue CA, CN=Fake Root",
            "common_name": "example.com",
            "name_value": "example.com",
            "serial_number": "04badc0de"
        });
        let rogue_entry = parse_crt_sh_entry(&rogue_json, "example.com", &allowed, true).unwrap();
        assert!(rogue_entry.is_rogue);
        assert!(
            rogue_entry
                .rogue_reason
                .unwrap()
                .contains("Untrusted or unauthorized CA")
        );
    }
}
