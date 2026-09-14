//! Sends a copy of the requests of a route to other servers.

use crate::error::Error;
use crate::proxy::Server;
use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The default copy body limit of 64 KiB.
pub const DEFAULT_MIRROR_BODY_SIZE: u64 = 64 * 1024;

/// Copies the requests of a route to other servers. r3v3rs3 drops the responses of these servers.
#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Mirror {
    /// Every server receives a copy of each mirrored request. The weight has no effect.
    #[serde(default)]
    pub servers: Vec<Server>,
    /// The share of the requests that r3v3rs3 copies, from 1 to 100.
    #[serde(
        default = "default_percent",
        skip_serializing_if = "is_default_percent"
    )]
    #[schema(example = 100)]
    pub percent: u8,
    /// The largest request body in bytes that r3v3rs3 copies. A request with a larger body is not
    /// copied.
    #[serde(
        default = "default_max_body_size",
        skip_serializing_if = "is_default_max_body_size"
    )]
    #[schema(example = 65536)]
    pub max_body_size: u64,
}

impl Mirror {
    /// Rejects a percent outside 1 to 100.
    pub fn validate(&self) -> Result<(), Error> {
        if (1..=100).contains(&self.percent) {
            Ok(())
        } else {
            Err(Error::InvalidMirrorPercent {
                percent: self.percent,
            })
        }
    }
}

fn default_percent() -> u8 {
    100
}

fn is_default_percent(percent: &u8) -> bool {
    *percent == default_percent()
}

fn default_max_body_size() -> u64 {
    DEFAULT_MIRROR_BODY_SIZE
}

fn is_default_max_body_size(size: &u64) -> bool {
    *size == DEFAULT_MIRROR_BODY_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_mirror_round_trips_and_rejects_an_invalid_percent() {
        let value = json!({
            "servers": [{ "url": "http://127.0.0.1:9000/" }],
            "percent": 25,
            "max_body_size": 1024,
        });
        let mirror: Mirror = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&mirror).unwrap(), value);
        assert!(mirror.validate().is_ok());

        let defaults: Mirror = serde_json::from_value(json!({ "servers": [] })).unwrap();
        assert_eq!(defaults.percent, 100);
        assert_eq!(defaults.max_body_size, DEFAULT_MIRROR_BODY_SIZE);
        assert_eq!(
            serde_json::to_value(&defaults).unwrap(),
            json!({ "servers": [] })
        );

        for percent in [0, 101] {
            let mirror = Mirror {
                percent,
                ..Default::default()
            };
            assert!(matches!(
                mirror.validate(),
                Err(Error::InvalidMirrorPercent { percent: value }) if value == percent
            ));
        }
    }
}
