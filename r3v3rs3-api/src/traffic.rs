use crate::id::ShortId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};

/// A single HTTP header entry (key-value pair).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
}

impl HeaderEntry {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// An HTTP access log record emitted by the proxy engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AccessLogEntry {
    pub id: u64,
    #[serde(
        serialize_with = "serialize_timestamp_ms",
        deserialize_with = "deserialize_timestamp_ms"
    )]
    #[schema(value_type = u64)]
    pub timestamp: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<ShortId>,
    pub client_ip: String,
    pub method: String,
    pub host: String,
    pub uri: String,
    pub status: u16,
    pub latency_ms: f64,
    pub proto: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_headers: Vec<HeaderEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_headers: Vec<HeaderEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_websocket: Option<bool>,
}

/// Request payload to replay an HTTP or WebSocket handshake request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RequestReplayRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<HeaderEntry>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Execution result of a replayed request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RequestReplayResponse {
    pub status: u16,
    pub latency_ms: f64,
    #[serde(default)]
    pub headers: Vec<HeaderEntry>,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn serialize_timestamp_ms<S>(timestamp: &OffsetDateTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let ms = (timestamp.unix_timestamp_nanos() / 1_000_000) as i64;
    serializer.serialize_i64(ms)
}

fn deserialize_timestamp_ms<'de, D>(deserializer: D) -> Result<OffsetDateTime, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let ms = i64::deserialize(deserializer)?;
    OffsetDateTime::from_unix_timestamp_nanos((ms as i128) * 1_000_000)
        .map_err(serde::de::Error::custom)
}

/// Query parameters for filtering access logs.
#[derive(Clone, Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AccessLogQuery {
    pub resource_id: Option<ShortId>,
    pub status: Option<u16>,
    pub search: Option<String>,
    pub limit: Option<usize>,
    pub since_id: Option<u64>,
    pub country: Option<String>,
}

/// Statistics of HTTP traffic originating from a specific country.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CountryTrafficStat {
    /// Two-letter ISO country code (e.g. "TR", "US", "DE") or "XX" for unresolved/private IPs.
    pub country: String,
    /// Total count of HTTP requests from this country.
    pub count: u64,
    /// Percentage of total resolved requests.
    pub percentage: f64,
}

/// Aggregated geographical distribution of access traffic across countries.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TrafficGeoDistribution {
    /// Total number of requests evaluated with valid country resolution.
    pub total_resolved: u64,
    /// Top countries ranked by request count (up to top 10).
    pub top_countries: Vec<CountryTrafficStat>,
    /// Full mapping of ISO country code to request count.
    pub country_counts: std::collections::BTreeMap<String, u64>,
}

impl AccessLogEntry {
    /// Checks whether this entry satisfies the given query filters.
    pub fn matches(&self, query: &AccessLogQuery) -> bool {
        self.matches_resource(query.resource_id.as_ref())
            && self.matches_status(query.status)
            && self.matches_since_id(query.since_id)
            && self.matches_search(query.search.as_deref())
            && self.matches_country(query.country.as_deref())
    }

    fn matches_resource(&self, resource_id: Option<&ShortId>) -> bool {
        match resource_id {
            Some(expected) => self.resource_id.as_ref() == Some(expected),
            None => true,
        }
    }

    fn matches_status(&self, status: Option<u16>) -> bool {
        match status {
            Some(expected) => self.status == expected,
            None => true,
        }
    }

    fn matches_since_id(&self, since_id: Option<u64>) -> bool {
        match since_id {
            Some(min_id) => self.id > min_id,
            None => true,
        }
    }

    fn matches_country(&self, country: Option<&str>) -> bool {
        match country {
            Some(expected) if !expected.trim().is_empty() => self
                .country
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(expected.trim())),
            _ => true,
        }
    }

    fn matches_search(&self, search: Option<&str>) -> bool {
        let Some(search) = search.map(str::trim).filter(|s| !s.is_empty()) else {
            return true;
        };
        let needle = search.to_lowercase();
        self.uri.to_lowercase().contains(&needle)
            || self.client_ip.to_lowercase().contains(&needle)
            || self.host.to_lowercase().contains(&needle)
            || self.method.to_lowercase().contains(&needle)
            || self.status.to_string().contains(&needle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(id: u64, status: u16, uri: &str) -> AccessLogEntry {
        AccessLogEntry {
            id,
            timestamp: OffsetDateTime::UNIX_EPOCH,
            resource_id: Some("app-prx".parse().unwrap()),
            client_ip: "192.168.1.100".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            uri: uri.to_string(),
            status,
            latency_ms: 12.5,
            proto: "HTTP/2.0".to_string(),
            error: None,
            country: None,
            request_headers: vec![HeaderEntry::new("User-Agent", "r3v3rs3-test")],
            response_headers: vec![HeaderEntry::new("Content-Type", "application/json")],
            request_body: None,
            response_body: None,
            content_type: Some("application/json".to_string()),
            is_websocket: Some(false),
        }
    }

    #[test]
    fn serde_roundtrip() {
        let entry = sample_entry(1, 200, "/api/v1/health");
        let json = serde_json::to_string(&entry).unwrap();
        let decoded: AccessLogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, decoded);
    }

    #[test]
    fn query_matching_status() {
        let entry = sample_entry(1, 404, "/not-found");
        let mut query = AccessLogQuery::default();
        assert!(entry.matches(&query));

        query.status = Some(404);
        assert!(entry.matches(&query));

        query.status = Some(200);
        assert!(!entry.matches(&query));
    }

    #[test]
    fn query_matching_search() {
        let entry = sample_entry(10, 200, "/v1/users/42");
        let query = AccessLogQuery {
            search: Some("users".to_string()),
            ..Default::default()
        };
        assert!(entry.matches(&query));

        let query_ip = AccessLogQuery {
            search: Some("192.168.1.100".to_string()),
            ..Default::default()
        };
        assert!(entry.matches(&query_ip));

        let query_miss = AccessLogQuery {
            search: Some("products".to_string()),
            ..Default::default()
        };
        assert!(!entry.matches(&query_miss));
    }

    #[test]
    fn query_matching_since_id() {
        let entry = sample_entry(5, 200, "/");
        assert!(entry.matches(&AccessLogQuery {
            since_id: Some(4),
            ..Default::default()
        }));
        assert!(!entry.matches(&AccessLogQuery {
            since_id: Some(5),
            ..Default::default()
        }));
    }

    #[test]
    fn query_matching_country() {
        let mut entry = sample_entry(6, 200, "/");
        entry.country = Some("TR".to_string());

        assert!(entry.matches(&AccessLogQuery {
            country: Some("TR".to_string()),
            ..Default::default()
        }));
        assert!(entry.matches(&AccessLogQuery {
            country: Some("tr".to_string()),
            ..Default::default()
        }));
        assert!(!entry.matches(&AccessLogQuery {
            country: Some("US".to_string()),
            ..Default::default()
        }));

        let no_country = sample_entry(7, 200, "/");
        assert!(!no_country.matches(&AccessLogQuery {
            country: Some("TR".to_string()),
            ..Default::default()
        }));
    }

    #[test]
    fn geo_distribution_serialization() {
        let mut counts = std::collections::BTreeMap::new();
        counts.insert("TR".to_string(), 100);
        counts.insert("DE".to_string(), 50);

        let geo = TrafficGeoDistribution {
            total_resolved: 150,
            top_countries: vec![
                CountryTrafficStat {
                    country: "TR".to_string(),
                    count: 100,
                    percentage: 66.67,
                },
                CountryTrafficStat {
                    country: "DE".to_string(),
                    count: 50,
                    percentage: 33.33,
                },
            ],
            country_counts: counts,
        };

        let json = serde_json::to_string(&geo).unwrap();
        let decoded: TrafficGeoDistribution = serde_json::from_str(&json).unwrap();
        assert_eq!(geo, decoded);
    }

    #[test]
    fn replay_models_serde_roundtrip() {
        let req = RequestReplayRequest {
            method: "POST".to_string(),
            url: "https://example.com/api/echo".to_string(),
            headers: vec![HeaderEntry::new("Authorization", "Bearer token123")],
            body: Some(r#"{"hello":"world"}"#.to_string()),
            timeout_ms: Some(5000),
        };
        let req_json = serde_json::to_string(&req).unwrap();
        let req_decoded: RequestReplayRequest = serde_json::from_str(&req_json).unwrap();
        assert_eq!(req, req_decoded);

        let resp = RequestReplayResponse {
            status: 200,
            latency_ms: 45.2,
            headers: vec![HeaderEntry::new("Content-Type", "application/json")],
            body: r#"{"ok":true}"#.to_string(),
            error: None,
        };
        let resp_json = serde_json::to_string(&resp).unwrap();
        let resp_decoded: RequestReplayResponse = serde_json::from_str(&resp_json).unwrap();
        assert_eq!(resp, resp_decoded);
    }
}
