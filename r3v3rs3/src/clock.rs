//! The wall clock time that the nodes of a cluster compare.

use std::time::{SystemTime, UNIX_EPOCH};

/// The Unix time in milliseconds. A clock before the Unix epoch gives zero.
pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}
