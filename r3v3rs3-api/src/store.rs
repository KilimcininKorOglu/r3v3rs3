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

/// An installable app of the catalog: a single image, or a Compose file of the r3v3rs3
/// repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
    /// The category of the app (e.g. "Monitoring", "Development", "Productivity").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// The Turkish category name. Empty or None means the English category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_tr: Option<String>,
    /// Optional screenshot URL or image path demonstrating the application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
    /// The container image, for example `nginx:1.27`. Set on an image entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// The HTTP port inside the container.
    pub port: u16,
    /// The path of the health check, starting with `/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check_path: Option<String>,
    /// The home page of the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    /// The clone URL of the repository that holds the Compose file. Set on a Compose entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// The branch of the Compose repository. The default is `main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The Compose file in the repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The service of the Compose file that serves the traffic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// The environment variables that the user fills before the first deployment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<StoreEnv>,
}

/// An environment variable of a Compose entry that the user fills in the panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StoreEnv {
    /// The name of the variable, for example `DB_PASSWORD`.
    pub key: String,
    /// The English description of what the variable holds.
    pub description: String,
    /// The Turkish description. Empty means the English description.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description_tr: String,
    /// The panel hides the value of a secret variable.
    #[serde(default)]
    pub secret: bool,
}

/// The text in the given language; the English one when no Turkish one exists.
fn description_in<'a>(tr: bool, text: &'a str, turkish: &'a str) -> &'a str {
    if tr && !turkish.is_empty() {
        turkish
    } else {
        text
    }
}

impl StoreApp {
    /// Whether the entry installs through a Compose file instead of a single image.
    pub fn is_compose(&self) -> bool {
        self.image.is_none()
    }

    /// The description in the given language; the English one when no Turkish one exists.
    pub fn description_in(&self, tr: bool) -> &str {
        description_in(tr, &self.description, &self.description_tr)
    }

    /// The category in the given language; the English one when no Turkish one exists.
    pub fn category_in(&self, tr: bool) -> Option<&str> {
        if tr
            && let Some(cat_tr) = self.category_tr.as_deref().filter(|s| !s.is_empty())
        {
            return Some(cat_tr);
        }
        self.category.as_deref()
    }
}

impl StoreEnv {
    /// The description in the given language; the English one when no Turkish one exists.
    pub fn description_in(&self, tr: bool) -> &str {
        description_in(tr, &self.description, &self.description_tr)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_in_respects_language_preference_and_fallback() {
        let app = StoreApp {
            category: Some("Monitoring".to_string()),
            category_tr: Some("İzleme".to_string()),
            ..StoreApp::default()
        };
        assert_eq!(app.category_in(false), Some("Monitoring"));
        assert_eq!(app.category_in(true), Some("İzleme"));

        let fallback_app = StoreApp {
            category: Some("Security".to_string()),
            category_tr: None,
            ..StoreApp::default()
        };
        assert_eq!(fallback_app.category_in(false), Some("Security"));
        assert_eq!(fallback_app.category_in(true), Some("Security"));

        let empty_tr_app = StoreApp {
            category: Some("Development".to_string()),
            category_tr: Some(String::new()),
            ..StoreApp::default()
        };
        assert_eq!(empty_tr_app.category_in(true), Some("Development"));

        let no_cat = StoreApp::default();
        assert_eq!(no_cat.category_in(false), None);
        assert_eq!(no_cat.category_in(true), None);
    }
}
