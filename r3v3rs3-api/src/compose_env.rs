//! Automatic Docker Compose and application environment variable template resolver and secret generator.

use crate::platform::EnvEntry;
use serde_derive::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request to generate secrets for an app's environment variables.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GenerateAppEnvSecretsRequest {
    /// Whether to save the generated secrets directly to the app configuration. Default is true.
    #[serde(default = "default_true")]
    pub save: bool,
    /// Whether to overwrite existing non-empty values. Default is false.
    #[serde(default)]
    pub overwrite: bool,
}

const fn default_true() -> bool {
    true
}

/// Request to resolve an arbitrary environment template (.env text or entry list) and generate secrets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResolveEnvTemplateRequest {
    /// Optional raw .env template string (e.g. `POSTGRES_PASSWORD=\nAPP_KEY=`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Optional structured list of environment entries to resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<EnvEntry>>,
    /// Whether to overwrite existing non-empty values. Default is false.
    #[serde(default)]
    pub overwrite: bool,
}

/// Response containing resolved environment variables and generated secret details.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResolveEnvTemplateResponse {
    /// The resolved .env text if a template string or entries were provided.
    pub template: String,
    /// The resolved structured environment entries.
    pub entries: Vec<EnvEntry>,
    /// Number of secrets generated.
    pub generated_count: usize,
    /// List of variable names for which secrets were generated.
    pub generated_keys: Vec<String>,
}

/// Checks whether an environment variable name implies a secret, password, key, or token.
pub fn is_secret_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    is_secret_suffix(&upper) || is_secret_exact_or_substr(&upper)
}

const SECRET_SUFFIXES: &[&str] = &[
    "_PASSWORD",
    "_PASSWD",
    "_PWD",
    "_SECRET",
    "_SECRET_KEY",
    "_KEY",
    "_TOKEN",
    "_SALT",
    "_HASH",
    "_PASSPHRASE",
    "_PRIVATE_KEY",
];

fn is_secret_suffix(upper: &str) -> bool {
    SECRET_SUFFIXES.iter().any(|suffix| upper.ends_with(suffix))
}

const SECRET_SUBSTRINGS: &[&str] = &[
    "JWT_SECRET",
    "AUTH_SECRET",
    "ENCRYPTION_KEY",
    "ROOT_PASSWORD",
    "ADMIN_PASSWORD",
    "DB_PASSWORD",
];

fn is_secret_exact_or_substr(upper: &str) -> bool {
    is_exact_secret_word(upper)
        || SECRET_SUBSTRINGS
            .iter()
            .any(|substr| upper.contains(substr))
}

fn is_exact_secret_word(upper: &str) -> bool {
    matches!(
        upper,
        "PASSWORD"
            | "PASSWD"
            | "PWD"
            | "SECRET"
            | "SECRET_KEY"
            | "KEY"
            | "TOKEN"
            | "SALT"
            | "HASH"
            | "PASSPHRASE"
            | "PRIVATE_KEY"
    )
}

/// Checks whether a variable value is empty or contains a common placeholder.
pub fn is_empty_or_placeholder(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }
    let unquoted = trimmed.trim_matches('\'').trim_matches('"').trim();
    if unquoted.is_empty() {
        return true;
    }
    is_placeholder_word(&unquoted.to_ascii_lowercase()) || is_bracket_placeholder(unquoted)
}

fn is_bracket_placeholder(unquoted: &str) -> bool {
    (unquoted.starts_with('<') && unquoted.ends_with('>'))
        || (unquoted.starts_with("${") && unquoted.ends_with('}'))
}

fn is_placeholder_word(lower: &str) -> bool {
    matches!(
        lower,
        "changeme"
            | "change_me"
            | "replace_me"
            | "replaceme"
            | "your_secret_here"
            | "your-secret-here"
            | "your_password_here"
            | "generate_secret"
            | "secret"
            | "password"
            | "todo"
            | "fixme"
            | "xxx"
            | "xxxx"
            | "<secret>"
            | "<password>"
            | "<replace_me>"
    )
}

/// Resolves a raw `.env` template string, generating secrets for empty or placeholder secrets.
pub fn resolve_env_template(
    template: &str,
    overwrite: bool,
    generator: impl Fn(&str) -> String,
) -> (String, Vec<EnvEntry>, Vec<String>) {
    let mut resolved_lines = Vec::new();
    let mut entries = Vec::new();
    let mut generated_keys = Vec::new();

    for line in template.lines() {
        let (resolved_line, entry_opt, key_opt) =
            process_template_line(line, overwrite, &generator);
        resolved_lines.push(resolved_line);
        if let Some(entry) = entry_opt {
            entries.push(entry);
        }
        if let Some(key) = key_opt {
            generated_keys.push(key);
        }
    }

    (resolved_lines.join("\n"), entries, generated_keys)
}

fn process_template_line(
    line: &str,
    overwrite: bool,
    generator: &impl Fn(&str) -> String,
) -> (String, Option<EnvEntry>, Option<String>) {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return (line.to_string(), None, None);
    }

    let prefix = if line.starts_with("export ") {
        "export "
    } else {
        ""
    };
    let content = line.strip_prefix("export ").unwrap_or(line);
    let Some((key_str, val_str)) = content.split_once('=') else {
        return (line.to_string(), None, None);
    };

    let key = key_str.trim();
    let is_secret = is_secret_key(key);
    let val_trimmed = val_str.trim();
    let needs_generation = is_secret && (is_empty_or_placeholder(val_trimmed) || overwrite);

    let (final_val, generated_key) = if needs_generation {
        let secret_val = generator(key);
        (secret_val, Some(key.to_string()))
    } else {
        (extract_clean_value(val_trimmed), None)
    };

    let Ok(env_key) = key.parse() else {
        return (line.to_string(), None, None);
    };

    let new_line = format!("{prefix}{key}={final_val}");
    let entry = EnvEntry {
        key: env_key,
        value: Some(final_val),
        secret: is_secret,
    };

    (new_line, Some(entry), generated_key)
}

fn extract_clean_value(val_str: &str) -> String {
    if (val_str.starts_with('"') && val_str.ends_with('"') && val_str.len() >= 2)
        || (val_str.starts_with('\'') && val_str.ends_with('\'') && val_str.len() >= 2)
    {
        val_str[1..val_str.len() - 1].to_string()
    } else {
        val_str
            .split_once(" #")
            .map_or(val_str, |(v, _)| v)
            .trim()
            .to_string()
    }
}

/// Resolves a slice of `EnvEntry`, generating values for missing or placeholder secrets.
pub fn resolve_env_entries(
    entries: &mut [EnvEntry],
    overwrite: bool,
    generator: impl Fn(&str) -> String,
) -> Vec<String> {
    let mut generated_keys = Vec::new();
    for entry in entries.iter_mut() {
        let is_secret = entry.secret || is_secret_key(entry.key.as_str());
        if !is_secret {
            continue;
        }
        let needs_val = entry
            .value
            .as_ref()
            .map(|v| is_empty_or_placeholder(v))
            .unwrap_or(true)
            || overwrite;

        if needs_val {
            let secret_val = generator(entry.key.as_str());
            entry.value = Some(secret_val);
            entry.secret = true;
            generated_keys.push(entry.key.as_str().to_string());
        }
    }
    generated_keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_secret_key() {
        assert!(is_secret_key("POSTGRES_PASSWORD"));
        assert!(is_secret_key("APP_SECRET"));
        assert!(is_secret_key("DIRECTUS_KEY"));
        assert!(is_secret_key("JWT_SECRET"));
        assert!(is_secret_key("DATABASE_URL_TOKEN"));
        assert!(is_secret_key("DB_PASSWORD"));
        assert!(!is_secret_key("SITE_URL"));
        assert!(!is_secret_key("APP_PORT"));
        assert!(!is_secret_key("NODE_ENV"));
    }

    #[test]
    fn test_is_empty_or_placeholder() {
        assert!(is_empty_or_placeholder(""));
        assert!(is_empty_or_placeholder("   "));
        assert!(is_empty_or_placeholder("\"\""));
        assert!(is_empty_or_placeholder("''"));
        assert!(is_empty_or_placeholder("changeme"));
        assert!(is_empty_or_placeholder("\"replace_me\""));
        assert!(is_empty_or_placeholder("<secret>"));
        assert!(is_empty_or_placeholder("${POSTGRES_PASSWORD}"));
        assert!(!is_empty_or_placeholder(
            "valid_production_secret_123456789"
        ));
        assert!(!is_empty_or_placeholder("https://example.com"));
    }

    #[test]
    fn test_resolve_env_template() {
        let template = r#"# Configuration
SITE_URL=https://myapp.com
POSTGRES_PASSWORD=
APP_SECRET=changeme
EXISTING_KEY=secret_preserved
"#;
        let (resolved, entries, generated) =
            resolve_env_template(template, false, |k| format!("gen_{k}"));

        assert_eq!(generated, vec!["POSTGRES_PASSWORD", "APP_SECRET"]);
        assert!(resolved.contains("SITE_URL=https://myapp.com"));
        assert!(resolved.contains("POSTGRES_PASSWORD=gen_POSTGRES_PASSWORD"));
        assert!(resolved.contains("APP_SECRET=gen_APP_SECRET"));
        assert!(resolved.contains("EXISTING_KEY=secret_preserved"));
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn test_resolve_env_entries() {
        let mut entries = vec![
            EnvEntry {
                key: "DB_PASSWORD".parse().unwrap(),
                value: None,
                secret: true,
            },
            EnvEntry {
                key: "API_URL".parse().unwrap(),
                value: Some("https://api.example.com".into()),
                secret: false,
            },
        ];

        let generated = resolve_env_entries(&mut entries, false, |_| "secret123".into());
        assert_eq!(generated, vec!["DB_PASSWORD"]);
        assert_eq!(entries[0].value.as_deref(), Some("secret123"));
        assert_eq!(entries[1].value.as_deref(), Some("https://api.example.com"));
    }
}
