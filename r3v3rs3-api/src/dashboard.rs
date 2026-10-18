//! The types of the dashboard summary overview.

use crate::id::ShortId;
use crate::platform::{DeploymentStatus, DeploymentTrigger};
use serde_derive::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The single-screen overview of the system state, resources, and platform statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DashboardSummary {
    pub ports: PortsSummary,
    pub proxies: ProxiesSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps: Option<AppsSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub databases: Option<DatabasesSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<TargetsSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<DiskSpace>,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_available: Option<String>,
    #[serde(default)]
    pub recent_deployments: Vec<DeploymentSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
pub struct PortsSummary {
    pub total: usize,
    pub active: usize,
    pub error: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
pub struct ProxiesSummary {
    pub total: usize,
    pub active: usize,
    pub error: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
pub struct AppsSummary {
    pub total: usize,
    pub running: usize,
    pub stopped: usize,
    pub update_available: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
pub struct DatabasesSummary {
    pub total: usize,
    pub running: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
pub struct TargetsSummary {
    pub total: usize,
    pub online: usize,
    pub offline: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
pub struct DiskSpace {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
    pub used_percent: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeploymentSummary {
    pub id: ShortId,
    pub app_id: ShortId,
    pub app_name: String,
    pub status: DeploymentStatus,
    pub trigger: DeploymentTrigger,
    pub started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_summary_round_trip() {
        let summary = DashboardSummary {
            ports: PortsSummary {
                total: 4,
                active: 4,
                error: 0,
            },
            proxies: ProxiesSummary {
                total: 10,
                active: 9,
                error: 1,
            },
            apps: Some(AppsSummary {
                total: 5,
                running: 4,
                stopped: 1,
                update_available: 2,
            }),
            databases: Some(DatabasesSummary {
                total: 2,
                running: 2,
            }),
            targets: Some(TargetsSummary {
                total: 3,
                online: 2,
                offline: 1,
            }),
            disk: Some(DiskSpace {
                total_bytes: 100_000_000_000,
                available_bytes: 75_000_000_000,
                used_bytes: 25_000_000_000,
                used_percent: 25,
            }),
            version: "2.0.4".into(),
            update_available: Some("v2.0.5".into()),
            recent_deployments: vec![],
        };

        let json = serde_json::to_string(&summary).unwrap();
        let parsed: DashboardSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(summary, parsed);
    }
}
