//! The app store: the catalog of the image apps that the deployment platform installs in one
//! click. The catalog ships embedded in the binary and refreshes from the repository daily.

use arc_swap::{ArcSwap, Guard};
use once_cell::sync::Lazy;
use r3v3rs3_api::store::{StoreCatalog, StoreResponse, StoreSource, StoreStatus};
use std::sync::{Arc, Mutex};
use tracing::error;

pub mod fetch;

const EMBEDDED_CATALOG: &str = include_str!("../../data/app-store.json");

pub(crate) struct State {
    pub(crate) catalog: StoreCatalog,
    pub(crate) source: StoreSource,
}

static STATE: Lazy<ArcSwap<State>> = Lazy::new(|| ArcSwap::from_pointee(embedded_state()));
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

fn embedded_state() -> State {
    match serde_json::from_str::<StoreCatalog>(EMBEDDED_CATALOG) {
        Ok(catalog) => State {
            catalog,
            source: StoreSource::Embedded,
        },
        Err(err) => {
            error!(%err, "failed to parse the embedded app store catalog");
            State {
                catalog: StoreCatalog::default(),
                source: StoreSource::Embedded,
            }
        }
    }
}

/// Returns the active catalog and its source.
pub(crate) fn state() -> Guard<Arc<State>> {
    STATE.load()
}

/// Installs a downloaded catalog when it is newer than the active one.
pub fn install(catalog: StoreCatalog) -> bool {
    if catalog.updated_at <= STATE.load().catalog.updated_at {
        return false;
    }
    STATE.store(Arc::new(State {
        catalog,
        source: StoreSource::Downloaded,
    }));
    true
}

pub fn set_last_error(error: String) {
    *LAST_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = error;
}

pub fn status() -> StoreStatus {
    let state = STATE.load();
    StoreStatus {
        updated_at: state.catalog.updated_at,
        source: state.source,
        apps: state.catalog.apps.len(),
        error: LAST_ERROR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone(),
    }
}

/// The status plus the catalog itself, the body of the app store endpoints.
pub fn response() -> StoreResponse {
    StoreResponse {
        status: status(),
        catalog: state().catalog.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::store::StoreApp;

    fn app(id: &str, _updated: i64) -> StoreApp {
        StoreApp {
            id: id.to_string(),
            name: id.to_string(),
            description: "test".to_string(),
            description_tr: String::new(),
            image: format!("{id}:1"),
            port: 80,
            health_check_path: None,
            website: None,
        }
    }

    #[test]
    fn embedded_catalog_parses() {
        let catalog: StoreCatalog = serde_json::from_str(EMBEDDED_CATALOG).unwrap();
        assert!(
            catalog.apps.len() >= 30,
            "the embedded catalog is too small"
        );
        let mut ids: Vec<_> = catalog.apps.iter().map(|app| app.id.as_str()).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), total, "a catalog id appears twice");
        for id in &ids {
            assert!(
                !id.is_empty()
                    && id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "invalid catalog id: {id}"
            );
        }
        for entry in &catalog.apps {
            assert!(!entry.image.is_empty(), "an app has no image: {}", entry.id);
            assert_ne!(entry.port, 0, "an app has no port: {}", entry.id);
        }
    }

    #[test]
    fn install_keeps_the_newer_catalog() {
        let newer = StoreCatalog {
            version: 1,
            updated_at: i64::MAX,
            apps: vec![app("new", i64::MAX)],
        };
        assert!(install(newer.clone()));
        assert_eq!(state().catalog.apps[0].id, "new");
        // The same catalog again is not newer, so it is not installed.
        assert!(!install(newer));
    }

    #[test]
    fn description_falls_back_to_english() {
        let mut entry = app("a", 1);
        assert_eq!(entry.description_in(true), "test");
        entry.description_tr = "deneme".to_string();
        assert_eq!(entry.description_in(true), "deneme");
        assert_eq!(entry.description_in(false), "test");
    }
}
