//! Access to the language that the user selected. The texts are in `r3v3rs3-api/locales`.

use crate::preferences::PreferencesStore;
use r3v3rs3_api::i18n::Locale;
use yew::prelude::*;
use yewdux::prelude::*;

/// Returns the selected language. The component renders again when the language changes.
#[hook]
pub fn use_locale() -> Locale {
    *use_selector(|preferences: &PreferencesStore| preferences.locale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    const NAMESPACES: [&str; 41] = [
        "acme",
        "accounts",
        "anomalies",
        "apps",
        "auth",
        "backups",
        "metrics",
        "certs",
        "common",
        "compose",
        "dashboard",
        "databases",
        "deployments",
        "diagnostics",
        "dns_credentials",
        "discovery",
        "env_groups",
        "footer",
        "git_connections",
        "geoip",
        "grpc",
        "http_form",
        "log",
        "login",
        "nav",
        "period",
        "ports",
        "projects",
        "protocol",
        "proxies",
        "proxy_form",
        "registries",
        "settings",
        "state",
        "targets",
        "theme",
        "time",
        "traffic",
        "webhooks",
        "workers",
        "error",
    ];

    fn read_sources(dir: &Path, sources: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                read_sources(&path, sources);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                sources.push(fs::read_to_string(path).unwrap());
            }
        }
    }

    fn string_literals(source: &str) -> Vec<String> {
        let mut literals = Vec::new();
        let mut current: Option<String> = None;
        let mut escaped = false;
        for c in source.chars() {
            let Some(text) = current.as_mut() else {
                if c == '"' {
                    current = Some(String::new());
                }
                continue;
            };
            if escaped {
                text.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                literals.extend(current.take());
            } else {
                text.push(c);
            }
        }
        literals
    }

    fn is_key(text: &str) -> bool {
        let valid_chars = text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.');
        valid_chars
            && !text.ends_with('.')
            && text
                .split_once('.')
                .is_some_and(|(namespace, _)| NAMESPACES.contains(&namespace))
    }

    #[test]
    fn every_used_key_is_defined() {
        let mut sources = Vec::new();
        read_sources(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut sources,
        );
        let keys = sources
            .iter()
            .flat_map(|source| string_literals(source))
            .filter(|text| is_key(text))
            .collect::<Vec<_>>();
        assert!(keys.len() > 150, "found only {} keys", keys.len());
        for key in keys {
            assert_ne!(Locale::En.t(&key), key, "missing key {key}");
        }
    }

    #[test]
    fn every_referenced_asset_exists() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut sources = Vec::new();
        read_sources(&manifest_dir.join("src"), &mut sources);
        if let Ok(index_html) = std::fs::read_to_string(manifest_dir.join("index.html")) {
            sources.push(index_html);
        }
        let assets = sources
            .iter()
            .flat_map(|source| string_literals(source))
            .filter(|text| is_known_asset_path(text))
            .collect::<std::collections::HashSet<_>>();
        assert!(assets.len() >= 25, "found only {} assets", assets.len());
        for asset in assets {
            let rel = asset.trim_start_matches('/');
            let path = manifest_dir.join(rel);
            assert!(
                path.exists(),
                "referenced asset {asset} does not exist at {}",
                path.display()
            );
        }
    }

    fn is_known_asset_path(text: &str) -> bool {
        const EXTENSIONS: &[&str] = &[".svg", ".png", ".ico", ".xml", ".js", ".css"];
        text.starts_with("/assets/") && EXTENSIONS.iter().any(|ext| text.ends_with(ext))
    }

    #[test]
    fn every_icon_in_assets_is_valid() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let icons_dir = manifest_dir.join("assets").join("icons");
        let entries = std::fs::read_dir(&icons_dir).expect("read icons dir");
        let mut count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "svg") {
                count += 1;
                let content = std::fs::read_to_string(&path).expect("read svg");
                assert!(
                    content.contains("<svg"),
                    "missing <svg tag in {}",
                    path.display()
                );
                assert!(
                    content.contains("</svg>"),
                    "missing </svg> in {}",
                    path.display()
                );
            }
        }
        assert!(count >= 20, "found only {count} icons in assets/icons");
    }
}
