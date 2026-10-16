//! The app store: the catalog of the image apps that the deployment platform can install in one
//! click. The catalog lives in the repository (`data/app-store.json`), ships embedded in the
//! binary and refreshes from the repository daily.

use serde_derive::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The catalog of the installable apps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StoreCatalog {
    /// The schema version of the catalog.
    pub version: u32,
    /// Unix time in seconds of the last catalog change in the repository.
    pub updated_at: i64,
    pub apps: Vec<StoreApp>,
}

/// An installable image app of the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StoreApp {
    /// The short id of the app, lowercase letters, digits and dashes.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The English description.
    pub description: String,
    /// The Turkish description. Empty means the English description.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description_tr: String,
    /// The container image, for example `nginx:1.27`.
    pub image: String,
    /// The HTTP port inside the container.
    pub port: u16,
    /// The path of the health check, starting with `/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check_path: Option<String>,
    /// The home page of the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
}

impl StoreApp {
    /// The description in the given language; the English one when no Turkish one exists.
    pub fn description_in(&self, tr: bool) -> &str {
        if tr && !self.description_tr.is_empty() {
            &self.description_tr
        } else {
            &self.description
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StoreSource {
    /// The catalog compiled into the binary.
    #[default]
    Embedded,
    /// A catalog downloaded from the repository.
    Downloaded,
    /// The last download failed; the previous catalog is still in use.
    Failed,
}

/// The state of the catalog that the server holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StoreStatus {
    /// Unix time in seconds of the catalog in use.
    pub updated_at: i64,
    pub source: StoreSource,
    pub apps: usize,
    /// The error of the last refresh attempt.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// The response of the app store endpoints: the state plus the catalog itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StoreResponse {
    pub status: StoreStatus,
    pub catalog: StoreCatalog,
}
