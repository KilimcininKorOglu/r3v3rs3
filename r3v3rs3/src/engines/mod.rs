//! The database engine catalog: the engines that the Databases page installs. The catalog
//! ships embedded in the binary and refreshes from the repository daily.

pub mod fetch;

use r3v3rs3_api::platform::{EnginesCatalog, install_engines};
use tracing::{error, info};

const EMBEDDED_CATALOG: &str = include_str!("../../data/engines.json");

/// Installs the embedded catalog once at startup. A saved download survives it, because
/// `install_engines` keeps the newer `updated_at`.
pub fn install_embedded() {
    match serde_json::from_str::<EnginesCatalog>(EMBEDDED_CATALOG) {
        Ok(catalog) => {
            install_engines(catalog);
        }
        Err(err) => error!(%err, "failed to parse the embedded engine catalog"),
    }
}

/// Installs a downloaded catalog when it is newer than the active one.
pub fn install(catalog: EnginesCatalog) -> bool {
    if install_engines(catalog) {
        info!("refreshed the engine catalog");
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry carries the fields that a database and its backup need.
    #[test]
    fn embedded_catalog_parses() {
        let catalog: EnginesCatalog = serde_json::from_str(EMBEDDED_CATALOG).unwrap();
        assert!(install_engines(catalog.clone()));
        assert!(r3v3rs3_api::platform::engines().len() >= 5);
        let mut keys: Vec<_> = catalog
            .engines
            .iter()
            .map(|info| info.engine.as_str())
            .collect();
        keys.sort_unstable();
        let total = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), total, "an engine key appears twice");
        for engine in &catalog.engines {
            assert!(
                !engine.versions.is_empty(),
                "an engine has no versions: {}",
                engine.engine
            );
            assert_ne!(engine.container_port, 0, "an engine has no port");
        }
    }
}
