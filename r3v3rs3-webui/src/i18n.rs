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

    const NAMESPACES: [&str; 20] = [
        "acme",
        "auth",
        "certs",
        "common",
        "discovery",
        "footer",
        "http_form",
        "log",
        "login",
        "nav",
        "period",
        "ports",
        "protocol",
        "proxies",
        "proxy_form",
        "settings",
        "state",
        "theme",
        "time",
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
}
