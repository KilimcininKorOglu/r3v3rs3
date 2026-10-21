use crate::id::ShortId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};

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
}

impl AccessLogEntry {
    /// Checks whether this entry satisfies the given query filters.
    pub fn matches(&self, query: &AccessLogQuery) -> bool {
        self.matches_resource(query.resource_id.as_ref())
            && self.matches_status(query.status)
            && self.matches_since_id(query.since_id)
            && self.matches_search(query.search.as_deref())
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
}
