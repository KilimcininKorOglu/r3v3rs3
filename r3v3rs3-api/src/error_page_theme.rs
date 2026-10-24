// Copyright 2026 r3v3rs3 Authors
// SPDX-License-Identifier: MIT

use crate::error::Error;
use crate::error_page_templates::{TEMPLATES, render_template};
use crate::id::ShortId;
use crate::proxy::CustomErrorPage;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A reusable error page theme containing custom error pages for various HTTP status codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorPageTheme {
    /// Unique identifier of the error page theme.
    pub id: ShortId,
    /// Human-friendly display name of the theme.
    pub name: String,
    /// Optional description of the theme design or purpose.
    #[serde(default)]
    pub description: String,
    /// List of custom error pages mapped by HTTP status code.
    #[serde(default)]
    pub pages: Vec<CustomErrorPage>,
}

impl ErrorPageTheme {
    pub fn validate(&self) -> Result<(), Error> {
        if self.name.trim().is_empty() {
            return Err(Error::InvalidErrorPageTheme {
                reason: "theme name cannot be empty".to_string(),
            });
        }
        if self.name.len() > 100 {
            return Err(Error::InvalidErrorPageTheme {
                reason: "theme name is too long (max 100 characters)".to_string(),
            });
        }
        for page in &self.pages {
            page.validate()?;
        }
        Ok(())
    }

    /// Finds the error page for a specific status code within this theme.
    pub fn find_page(&self, status: u16) -> Option<&CustomErrorPage> {
        self.pages.iter().find(|p| p.status == status)
    }
}

/// Standard status codes generated for built-in themes.
pub const STANDARD_ERROR_STATUSES: &[u16] =
    &[400, 401, 403, 404, 405, 408, 429, 500, 502, 503, 504];

fn builtin_theme_id(template_id: &str) -> ShortId {
    use std::str::FromStr;
    match template_id {
        "modern_glass" => ShortId::from_str("glass").unwrap_or_default(),
        "cyber_terminal" => ShortId::from_str("term").unwrap_or_default(),
        "playful_illustration" => ShortId::from_str("card").unwrap_or_default(),
        "maintenance_status" => ShortId::from_str("maint").unwrap_or_default(),
        _ => ShortId::from_str("dark").unwrap_or_default(),
    }
}

/// Generates standard built-in themes populated with template designs.
pub fn default_builtin_themes() -> Vec<ErrorPageTheme> {
    TEMPLATES
        .iter()
        .map(|template| {
            let pages = STANDARD_ERROR_STATUSES
                .iter()
                .map(|&status| {
                    let body = render_template(template.id, status);
                    CustomErrorPage { status, body }
                })
                .collect();
            ErrorPageTheme {
                id: builtin_theme_id(template.id),
                name: template.name.to_string(),
                description: template.description.to_string(),
                pages,
            }
        })
        .collect()
}

/// Replaces dynamic variables in an error page body.
/// Supported placeholders:
/// - `{{status}}`: HTTP status code (e.g. 502)
/// - `{{message}}`: Status title or error text (e.g. Bad Gateway)
/// - `{{request_id}}`: Unique request tracing ID
/// - `{{host}}`: Request host
/// - `{{client_ip}}`: Client IP address
pub fn substitute_error_page_variables(
    body: &str,
    status: u16,
    message: &str,
    request_id: &str,
    host: &str,
    client_ip: &str,
) -> String {
    let mut out = body.replace("{{status}}", &status.to_string());
    out = out.replace("{{message}}", message);
    out = out.replace(
        "{{request_id}}",
        if request_id.is_empty() {
            "-"
        } else {
            request_id
        },
    );
    out = out.replace("{{host}}", if host.is_empty() { "-" } else { host });
    out = out.replace(
        "{{client_ip}}",
        if client_ip.is_empty() { "-" } else { client_ip },
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_error_page_variables() {
        let template = "<h1>{{status}} {{message}}</h1><p>ID: {{request_id}} | Host: {{host}} | IP: {{client_ip}}</p>";
        let rendered = substitute_error_page_variables(
            template,
            502,
            "Bad Gateway",
            "req-12345",
            "example.com",
            "192.168.1.100",
        );
        assert_eq!(
            rendered,
            "<h1>502 Bad Gateway</h1><p>ID: req-12345 | Host: example.com | IP: 192.168.1.100</p>"
        );
    }

    #[test]
    fn test_substitute_error_page_variables_empty_defaults() {
        let template = "Req: {{request_id}}, Host: {{host}}, IP: {{client_ip}}";
        let rendered = substitute_error_page_variables(template, 404, "Not Found", "", "", "");
        assert_eq!(rendered, "Req: -, Host: -, IP: -");
    }

    #[test]
    fn test_default_builtin_themes() {
        let themes = default_builtin_themes();
        assert_eq!(themes.len(), TEMPLATES.len());
        for theme in &themes {
            assert!(theme.validate().is_ok());
            assert_eq!(theme.pages.len(), STANDARD_ERROR_STATUSES.len());
            assert!(theme.find_page(404).is_some());
            assert!(theme.find_page(502).is_some());
            assert!(theme.find_page(503).is_some());
        }
    }

    #[test]
    fn test_theme_validation() {
        let valid = ErrorPageTheme {
            id: ShortId::new(),
            name: "My Theme".to_string(),
            description: "Test".to_string(),
            pages: vec![CustomErrorPage {
                status: 404,
                body: "Not Found".to_string(),
            }],
        };
        assert!(valid.validate().is_ok());

        let empty_name = ErrorPageTheme {
            name: "   ".to_string(),
            ..valid.clone()
        };
        assert!(empty_name.validate().is_err());

        let long_name = ErrorPageTheme {
            name: "a".repeat(101),
            ..valid.clone()
        };
        assert!(long_name.validate().is_err());
    }
}
