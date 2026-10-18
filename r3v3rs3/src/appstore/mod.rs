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

    fn app(id: &str) -> StoreApp {
        StoreApp {
            id: id.to_string(),
            name: id.to_string(),
            description: "test".to_string(),
            description_tr: String::new(),
            category: None,
            category_tr: None,
            screenshot: None,
            image: Some(format!("{id}:1")),
            port: 80,
            health_check_path: None,
            website: None,
            repository: None,
            branch: None,
            file: None,
            service: None,
            env: Vec::new(),
        }
    }

    /// Every entry is one kind: an image entry names only an image, a Compose entry names a
    /// repository, a file and a service, and its variables carry valid key names.
    #[test]
    fn embedded_catalog_parses() {
        let catalog: StoreCatalog = serde_json::from_str(EMBEDDED_CATALOG).unwrap();
        assert!(
            catalog.apps.len() >= 30,
            "the embedded catalog is too small"
        );
        assert!(
            catalog
                .apps
                .iter()
                .any(|app| app.is_compose() && !app.env.is_empty()),
            "the embedded catalog holds no Compose entry with variables"
        );
        let mut ids: Vec<_> = catalog.apps.iter().map(|app| app.id.as_str()).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), total, "a catalog id appears twice");
        for entry in &catalog.apps {
            let id = &entry.id;
            assert!(
                !id.is_empty()
                    && id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "invalid catalog id: {id}"
            );
            assert_ne!(entry.port, 0, "an app has no port: {id}");
            if entry.is_compose() {
                assert_compose_entry(entry);
            } else {
                assert_image_entry(entry);
            }
            for var in &entry.env {
                assert!(
                    !var.key.is_empty()
                        && var
                            .key
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                        && var
                            .key
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                    "an entry has an invalid variable key: {id}/{}",
                    var.key
                );
            }
        }
    }

    fn assert_image_entry(entry: &StoreApp) {
        assert!(
            !entry.image.as_deref().unwrap_or_default().is_empty(),
            "an app has no image: {}",
            entry.id
        );
        assert!(
            entry.repository.is_none(),
            "an image entry names a repo: {}",
            entry.id
        );
    }

    fn assert_compose_entry(entry: &StoreApp) {
        assert!(
            entry.image.is_none(),
            "a Compose entry names an image: {}",
            entry.id
        );
        assert!(
            entry
                .repository
                .as_deref()
                .unwrap_or_default()
                .starts_with("https://")
                && entry.branch.as_deref().unwrap_or_default() == "main"
                && entry
                    .file
                    .as_deref()
                    .unwrap_or_default()
                    .ends_with("compose.yaml")
                && entry.service.as_deref().unwrap_or_default() == entry.id,
            "the Compose entry is incomplete: {}",
            entry.id
        );
    }

    #[test]
    fn install_keeps_the_newer_catalog() {
        let newer = StoreCatalog {
            version: 1,
            updated_at: i64::MAX,
            apps: vec![app("new")],
        };
        assert!(install(newer.clone()));
        assert_eq!(state().catalog.apps[0].id, "new");
        // The same catalog again is not newer, so it is not installed.
        assert!(!install(newer));
    }

    #[test]
    fn description_falls_back_to_english() {
        let mut entry = app("a");
        assert_eq!(entry.description_in(true), "test");
        entry.description_tr = "deneme".to_string();
        assert_eq!(entry.description_in(true), "deneme");
        assert_eq!(entry.description_in(false), "test");
    }

    #[test]
    fn embedded_catalog_has_categories_and_screenshots() {
        let catalog: StoreCatalog = serde_json::from_str(EMBEDDED_CATALOG).unwrap();
        for entry in &catalog.apps {
            let cat = entry.category.as_deref().unwrap_or_default();
            assert!(!cat.is_empty(), "entry {} has no category", entry.id);
            let cat_tr = entry.category_tr.as_deref().unwrap_or_default();
            assert!(!cat_tr.is_empty(), "entry {} has no category_tr", entry.id);
        }
        assert!(
            catalog.apps.iter().any(|app| app.screenshot.is_some()),
            "catalog has no screenshots"
        );
    }
}
