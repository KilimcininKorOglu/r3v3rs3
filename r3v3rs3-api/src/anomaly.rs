//! Data types for Prometheus anomaly detection and system alerts.

use crate::error::Error;
use serde_derive::{Deserialize, Serialize};
use utoipa::ToSchema;

fn default_true() -> bool {
    true
}

/// The anomaly detection alert rules and threshold configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AnomalyAlertRules {
    /// Whether anomaly threshold monitoring and alerts are enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// High P99 latency threshold in milliseconds (e.g. 500 for > 500 ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_p99_latency_ms: Option<u32>,
    /// High 5xx error rate threshold as a percentage of requests (e.g. 5 for > 5%).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_5xx_percent: Option<u8>,
    /// Traffic spike threshold in requests per second (RPS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_request_rate_rps: Option<u32>,
    /// Tokio runtime event loop lag threshold in milliseconds (e.g. 100 for > 100 ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_event_loop_lag_ms: Option<u32>,
}

impl Default for AnomalyAlertRules {
    fn default() -> Self {
        Self {
            enabled: true,
            max_p99_latency_ms: Some(1000),
            max_5xx_percent: Some(5),
            max_request_rate_rps: None,
            max_event_loop_lag_ms: Some(150),
        }
    }
}

impl AnomalyAlertRules {
    pub fn validate(&self) -> Result<(), Error> {
        if let Some(lat) = self.max_p99_latency_ms
            && lat == 0
        {
            return Err(Error::InvalidAnomalyAlertRules {
                reason: "max_p99_latency_ms must be greater than 0".to_string(),
            });
        }
        if let Some(err_pct) = self.max_5xx_percent
            && (err_pct == 0 || err_pct > 100)
        {
            return Err(Error::InvalidAnomalyAlertRules {
                reason: "max_5xx_percent must be between 1 and 100".to_string(),
            });
        }
        if let Some(rps) = self.max_request_rate_rps
            && rps == 0
        {
            return Err(Error::InvalidAnomalyAlertRules {
                reason: "max_request_rate_rps must be greater than 0".to_string(),
            });
        }
        if let Some(lag) = self.max_event_loop_lag_ms
            && lag == 0
        {
            return Err(Error::InvalidAnomalyAlertRules {
                reason: "max_event_loop_lag_ms must be greater than 0".to_string(),
            });
        }
        Ok(())
    }
}

/// The classification of a system or proxy anomaly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyKind {
    HighLatency,
    HighErrorRate,
    TrafficSpike,
    EventLoopLag,
}

impl AnomalyKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HighLatency => "high_latency",
            Self::HighErrorRate => "high_error_rate",
            Self::TrafficSpike => "traffic_spike",
            Self::EventLoopLag => "event_loop_lag",
        }
    }
}

/// The severity level of an anomaly alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnomalySeverity {
    Warning,
    Critical,
}

/// An active or historical anomaly alert record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AnomalyAlert {
    pub id: String,
    pub kind: AnomalyKind,
    pub severity: AnomalySeverity,
    pub summary: String,
    pub current_value: f64,
    pub threshold_value: f64,
    pub unit: String,
    pub triggered_at: i64,
}

/// An aggregated real-time snapshot of system anomaly monitoring.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SystemAnomaliesSnapshot {
    pub rules: AnomalyAlertRules,
    pub active_anomalies: Vec<AnomalyAlert>,
    pub recent_anomalies: Vec<AnomalyAlert>,
    pub p99_latency_ms: f64,
    pub error_rate_5xx_percent: f64,
    pub current_rps: f64,
    pub event_loop_lag_ms: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rules_default_and_validation() {
        let rules = AnomalyAlertRules::default();
        assert!(rules.enabled);
        assert_eq!(rules.max_p99_latency_ms, Some(1000));
        assert_eq!(rules.max_5xx_percent, Some(5));
        assert!(rules.validate().is_ok());

        let invalid_latency = AnomalyAlertRules {
            max_p99_latency_ms: Some(0),
            ..rules
        };
        assert!(invalid_latency.validate().is_err());

        let invalid_5xx = AnomalyAlertRules {
            max_5xx_percent: Some(101),
            ..rules
        };
        assert!(invalid_5xx.validate().is_err());

        let invalid_rps = AnomalyAlertRules {
            max_request_rate_rps: Some(0),
            ..rules
        };
        assert!(invalid_rps.validate().is_err());

        let invalid_lag = AnomalyAlertRules {
            max_event_loop_lag_ms: Some(0),
            ..rules
        };
        assert!(invalid_lag.validate().is_err());
    }

    #[test]
    fn test_anomaly_snapshot_roundtrip() {
        let alert = AnomalyAlert {
            id: "latency-1".to_string(),
            kind: AnomalyKind::HighLatency,
            severity: AnomalySeverity::Critical,
            summary: "P99 latency is 1250ms".to_string(),
            current_value: 1250.0,
            threshold_value: 1000.0,
            unit: "ms".to_string(),
            triggered_at: 1700000000,
        };
        let snapshot = SystemAnomaliesSnapshot {
            rules: AnomalyAlertRules::default(),
            active_anomalies: vec![alert.clone()],
            recent_anomalies: vec![alert],
            p99_latency_ms: 1250.0,
            error_rate_5xx_percent: 1.2,
            current_rps: 450.0,
            event_loop_lag_ms: 12.0,
        };
        let serialized = serde_json::to_string(&snapshot).unwrap();
        let deserialized: SystemAnomaliesSnapshot = serde_json::from_str(&serialized).unwrap();
        assert_eq!(snapshot, deserialized);
    }
}
