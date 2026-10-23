//! Outbound webhooks and event notification types.

use crate::error::Error;
use crate::id::ShortId;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use utoipa::ToSchema;

fn default_true() -> bool {
    true
}

/// The category of a webhook event for grouping in UI and documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEventCategory {
    Deployment,
    Backup,
    Security,
    System,
    Test,
}

impl WebhookEventCategory {
    pub const ALL: [WebhookEventCategory; 5] = [
        Self::Deployment,
        Self::Backup,
        Self::Security,
        Self::System,
        Self::Test,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Deployment => "deployment",
            Self::Backup => "backup",
            Self::Security => "security",
            Self::System => "system",
            Self::Test => "test",
        }
    }
}

/// One event type that r3v3rs3 can publish to outbound webhooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ToSchema)]
pub enum WebhookEvent {
    // Deployment events
    DeploySuccess,
    DeployFailure,
    DeployPendingApproval,
    AppCrashed,

    // Database & Backup events
    BackupSuccess,
    BackupFailure,
    BackupUploadFailed,
    BackupRestored,
    BackupVerificationFailed,

    // Security & Certificate events
    CertExpiring,
    CertRenewed,
    CertRenewalFailed,
    SecurityIpBanned,
    SecurityThreatDetected,

    // Node & System events
    TargetOffline,
    SystemDiskPressure,
    SystemDiskLow,
    SystemBandwidthSpike,
    SystemHighLatency,
    SystemHighErrorRate,
    SystemTrafficSpike,
    SystemEventLoopLag,
    SystemFdExhaustion,
    SystemSocketExhaustion,
    SystemGeoipUpdated,
    SystemGeoipUpdateFailed,

    // Test ping
    TestPing,
}

impl WebhookEvent {
    pub const ALL: [WebhookEvent; 27] = [
        Self::DeploySuccess,
        Self::DeployFailure,
        Self::DeployPendingApproval,
        Self::AppCrashed,
        Self::BackupSuccess,
        Self::BackupFailure,
        Self::BackupUploadFailed,
        Self::BackupRestored,
        Self::BackupVerificationFailed,
        Self::CertExpiring,
        Self::CertRenewed,
        Self::CertRenewalFailed,
        Self::SecurityIpBanned,
        Self::SecurityThreatDetected,
        Self::TargetOffline,
        Self::SystemDiskPressure,
        Self::SystemDiskLow,
        Self::SystemBandwidthSpike,
        Self::SystemHighLatency,
        Self::SystemHighErrorRate,
        Self::SystemTrafficSpike,
        Self::SystemEventLoopLag,
        Self::SystemFdExhaustion,
        Self::SystemSocketExhaustion,
        Self::SystemGeoipUpdated,
        Self::SystemGeoipUpdateFailed,
        Self::TestPing,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DeploySuccess => "deploy.success",
            Self::DeployFailure => "deploy.failure",
            Self::DeployPendingApproval => "deploy.pending_approval",
            Self::AppCrashed => "app.crashed",
            Self::BackupSuccess => "backup.success",
            Self::BackupFailure => "backup.failure",
            Self::BackupUploadFailed => "backup.upload_failed",
            Self::BackupRestored => "backup.restored",
            Self::BackupVerificationFailed => "backup.verification_failed",
            Self::CertExpiring => "cert.expiring",
            Self::CertRenewed => "cert.renewed",
            Self::CertRenewalFailed => "cert.renewal_failed",
            Self::SecurityIpBanned => "security.ip_banned",
            Self::SecurityThreatDetected => "security.threat_detected",
            Self::TargetOffline => "target.offline",
            Self::SystemDiskPressure => "system.disk_pressure",
            Self::SystemDiskLow => "system.disk_low",
            Self::SystemBandwidthSpike => "system.bandwidth_spike",
            Self::SystemHighLatency => "system.high_latency",
            Self::SystemHighErrorRate => "system.high_error_rate",
            Self::SystemTrafficSpike => "system.traffic_spike",
            Self::SystemEventLoopLag => "system.event_loop_lag",
            Self::SystemFdExhaustion => "system.fd_exhaustion",
            Self::SystemSocketExhaustion => "system.socket_exhaustion",
            Self::SystemGeoipUpdated => "system.geoip_updated",
            Self::SystemGeoipUpdateFailed => "system.geoip_update_failed",
            Self::TestPing => "test.ping",
        }
    }

    pub fn category(&self) -> WebhookEventCategory {
        match self {
            Self::DeploySuccess
            | Self::DeployFailure
            | Self::DeployPendingApproval
            | Self::AppCrashed => WebhookEventCategory::Deployment,
            Self::BackupSuccess
            | Self::BackupFailure
            | Self::BackupUploadFailed
            | Self::BackupRestored
            | Self::BackupVerificationFailed => WebhookEventCategory::Backup,
            Self::CertExpiring
            | Self::CertRenewed
            | Self::CertRenewalFailed
            | Self::SecurityIpBanned
            | Self::SecurityThreatDetected => WebhookEventCategory::Security,
            Self::TargetOffline
            | Self::SystemDiskPressure
            | Self::SystemDiskLow
            | Self::SystemBandwidthSpike
            | Self::SystemHighLatency
            | Self::SystemHighErrorRate
            | Self::SystemTrafficSpike
            | Self::SystemEventLoopLag
            | Self::SystemFdExhaustion
            | Self::SystemSocketExhaustion
            | Self::SystemGeoipUpdated
            | Self::SystemGeoipUpdateFailed => WebhookEventCategory::System,
            Self::TestPing => WebhookEventCategory::Test,
        }
    }
}

impl fmt::Display for WebhookEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for WebhookEvent {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "deploy.success" => Ok(Self::DeploySuccess),
            "deploy.failure" => Ok(Self::DeployFailure),
            "deploy.pending_approval" => Ok(Self::DeployPendingApproval),
            "app.crashed" => Ok(Self::AppCrashed),
            "backup.success" => Ok(Self::BackupSuccess),
            "backup.failure" => Ok(Self::BackupFailure),
            "backup.upload_failed" => Ok(Self::BackupUploadFailed),
            "backup.restored" => Ok(Self::BackupRestored),
            "backup.verification_failed" => Ok(Self::BackupVerificationFailed),
            "cert.expiring" => Ok(Self::CertExpiring),
            "cert.renewed" => Ok(Self::CertRenewed),
            "cert.renewal_failed" => Ok(Self::CertRenewalFailed),
            "security.ip_banned" => Ok(Self::SecurityIpBanned),
            "security.threat_detected" => Ok(Self::SecurityThreatDetected),
            "target.offline" => Ok(Self::TargetOffline),
            "system.disk_pressure" => Ok(Self::SystemDiskPressure),
            "system.disk_low" => Ok(Self::SystemDiskLow),
            "system.bandwidth_spike" => Ok(Self::SystemBandwidthSpike),
            "system.high_latency" => Ok(Self::SystemHighLatency),
            "system.high_error_rate" => Ok(Self::SystemHighErrorRate),
            "system.traffic_spike" => Ok(Self::SystemTrafficSpike),
            "system.event_loop_lag" => Ok(Self::SystemEventLoopLag),
            "system.fd_exhaustion" => Ok(Self::SystemFdExhaustion),
            "system.socket_exhaustion" => Ok(Self::SystemSocketExhaustion),
            "system.geoip_updated" => Ok(Self::SystemGeoipUpdated),
            "system.geoip_update_failed" => Ok(Self::SystemGeoipUpdateFailed),
            "test.ping" => Ok(Self::TestPing),
            _ => Err(Error::InvalidWebhook {
                reason: format!("unknown event '{s}'"),
            }),
        }
    }
}

impl serde::Serialize for WebhookEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for WebhookEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        WebhookEvent::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// A configured outbound webhook endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OutboundWebhook {
    pub id: ShortId,
    pub name: String,
    pub url: String,
    pub has_secret: bool,
    pub enabled: bool,
    pub events: Vec<WebhookEvent>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Request to create a new outbound webhook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateWebhookRequest {
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub events: Vec<WebhookEvent>,
}

impl CreateWebhookRequest {
    pub fn validate(&self) -> Result<(), Error> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(Error::InvalidWebhook {
                reason: "webhook name cannot be empty".to_string(),
            });
        }
        if name.len() > 100 {
            return Err(Error::InvalidWebhook {
                reason: "webhook name cannot exceed 100 characters".to_string(),
            });
        }
        validate_webhook_url(&self.url)?;
        if self.events.is_empty() {
            return Err(Error::InvalidWebhook {
                reason: "at least one event must be selected".to_string(),
            });
        }
        if let Some(ref sec) = self.secret
            && sec.len() > 256
        {
            return Err(Error::InvalidWebhook {
                reason: "secret cannot exceed 256 characters".to_string(),
            });
        }
        Ok(())
    }
}

/// Request to update an existing outbound webhook.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UpdateWebhookRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<WebhookEvent>>,
}

impl UpdateWebhookRequest {
    pub fn validate(&self) -> Result<(), Error> {
        if let Some(ref name) = self.name {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return Err(Error::InvalidWebhook {
                    reason: "webhook name cannot be empty".to_string(),
                });
            }
            if trimmed.len() > 100 {
                return Err(Error::InvalidWebhook {
                    reason: "webhook name cannot exceed 100 characters".to_string(),
                });
            }
        }
        if let Some(ref url) = self.url {
            validate_webhook_url(url)?;
        }
        if let Some(ref events) = self.events
            && events.is_empty()
        {
            return Err(Error::InvalidWebhook {
                reason: "at least one event must be selected".to_string(),
            });
        }
        if let Some(ref sec) = self.secret
            && sec.len() > 256
        {
            return Err(Error::InvalidWebhook {
                reason: "secret cannot exceed 256 characters".to_string(),
            });
        }
        Ok(())
    }
}

/// Validates that a string is a valid HTTP or HTTPS URL.
pub fn validate_webhook_url(url: &str) -> Result<(), Error> {
    let parsed = url::Url::parse(url.trim()).map_err(|err| Error::InvalidWebhook {
        reason: format!("invalid webhook URL: {err}"),
    })?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(Error::InvalidWebhook {
            reason: "webhook URL must use http or https scheme".to_string(),
        });
    }
    if parsed.host().is_none() {
        return Err(Error::InvalidWebhook {
            reason: "webhook URL must have a host".to_string(),
        });
    }
    Ok(())
}

/// Request to test an outbound webhook.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TestWebhookRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_id: Option<ShortId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<WebhookEvent>,
}

/// Result of testing an outbound webhook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TestWebhookResponse {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    pub message: String,
    pub elapsed_ms: u64,
}

/// Standard JSON payload delivered to outbound webhooks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WebhookPayload {
    /// Unique event delivery ID (e.g. `evt_...`).
    pub id: String,
    /// String identifier of the event (e.g. `deploy.success`).
    pub event: WebhookEvent,
    /// ISO 8601 UTC timestamp string.
    pub created_at: String,
    /// Unix timestamp in seconds.
    pub timestamp: i64,
    /// Human-readable plain text summary.
    pub summary: String,
    /// Contextual data payload specific to the event.
    pub data: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_roundtrip_and_categories() {
        for event in WebhookEvent::ALL {
            let s = event.as_str();
            let parsed: WebhookEvent = s.parse().unwrap();
            assert_eq!(parsed, event);

            let json = serde_json::to_string(&event).unwrap();
            let deserialized: WebhookEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, event);

            let _cat = event.category();
        }
    }

    #[test]
    fn validation_rules() {
        let req = CreateWebhookRequest {
            name: "Slack Ops".to_string(),
            url: "https://hooks.slack.com/services/xxx".to_string(),
            secret: Some("secret123".to_string()),
            enabled: true,
            events: vec![WebhookEvent::DeploySuccess, WebhookEvent::BackupFailure],
        };
        assert!(req.validate().is_ok());

        let invalid_url = CreateWebhookRequest {
            name: "Test".to_string(),
            url: "ftp://example.com".to_string(),
            secret: None,
            enabled: true,
            events: vec![WebhookEvent::TestPing],
        };
        assert!(invalid_url.validate().is_err());

        let empty_events = CreateWebhookRequest {
            name: "Test".to_string(),
            url: "https://example.com/webhook".to_string(),
            secret: None,
            enabled: true,
            events: vec![],
        };
        assert!(empty_events.validate().is_err());
    }
}
