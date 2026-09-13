use crate::error::Error;
use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use std::time::Duration;
use utoipa::ToSchema;

/// Stores upstream responses in memory and sends them without contacting the upstream server.
#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CacheConfig {
    #[serde(default)]
    pub enabled: bool,

    /// Memory limit of the stored responses of this proxy in bytes.
    #[serde(default = "default_max_size")]
    pub max_size: u64,

    /// Responses with a larger body in bytes are not stored.
    #[serde(default = "default_max_entry_size")]
    pub max_entry_size: u64,

    /// Freshness lifetime of a response without `Cache-Control: max-age`, `s-maxage` or `Expires`.
    /// Zero stores such a response only when it has an `ETag` or `Last-Modified` validator.
    #[serde(default, with = "humantime_serde")]
    #[schema(value_type = String, example = "5m")]
    pub default_ttl: Duration,
}

pub const DEFAULT_MAX_SIZE: u64 = 64 * 1024 * 1024;
pub const DEFAULT_MAX_ENTRY_SIZE: u64 = 1024 * 1024;

fn default_max_size() -> u64 {
    DEFAULT_MAX_SIZE
}

fn default_max_entry_size() -> u64 {
    DEFAULT_MAX_ENTRY_SIZE
}

impl CacheConfig {
    pub fn is_disabled(&self) -> bool {
        !self.enabled
    }

    pub fn validate(&self) -> Result<(), Error> {
        let valid = self.max_entry_size > 0
            && self.max_entry_size <= self.max_size
            && self.max_entry_size <= u64::from(u32::MAX);
        if self.enabled && !valid {
            Err(Error::InvalidCacheSize)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_fills_defaults() {
        let config: CacheConfig =
            serde_json::from_str(r#"{"enabled":true,"default_ttl":"5m"}"#).unwrap();
        assert_eq!(config.max_size, DEFAULT_MAX_SIZE);
        assert_eq!(config.max_entry_size, DEFAULT_MAX_ENTRY_SIZE);
        assert_eq!(config.default_ttl, Duration::from_secs(300));
        assert!(CacheConfig::default().is_disabled());
    }

    #[test]
    fn sizes_are_validated_when_enabled() {
        let config = |max_size, max_entry_size| CacheConfig {
            enabled: true,
            max_size,
            max_entry_size,
            ..Default::default()
        };
        assert!(config(1024, 1024).validate().is_ok());
        assert!(config(1024, 0).validate().is_err());
        assert!(config(1024, 2048).validate().is_err());
        assert!(config(u64::MAX, u64::from(u32::MAX) + 1)
            .validate()
            .is_err());
        let disabled = CacheConfig {
            enabled: false,
            ..config(1024, 0)
        };
        assert!(disabled.validate().is_ok());
    }
}
