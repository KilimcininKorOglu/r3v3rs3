//! Changes the request path before r3v3rs3 sends the request to an upstream server.

use crate::error::Error;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;
use utoipa::ToSchema;

/// Changes the path of the requests of a route. r3v3rs3 applies the steps in field order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PathRewrite {
    /// Removes the route path from the request path.
    #[serde(default = "default_strip_prefix", skip_serializing_if = "is_true")]
    pub strip_prefix: bool,

    /// Replaces the first match in the path with `replacement`. The path starts with `/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "^/items/([0-9]+)$")]
    pub regex: Option<PathRegex>,

    /// The text that replaces the match. `${1}` or `${name}` inserts a capture group.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[schema(example = "/item/${1}")]
    pub replacement: String,

    /// The path that r3v3rs3 adds before the path.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[schema(example = "/v2")]
    pub add_prefix: String,
}

impl Default for PathRewrite {
    fn default() -> Self {
        Self {
            strip_prefix: default_strip_prefix(),
            regex: None,
            replacement: String::new(),
            add_prefix: String::new(),
        }
    }
}

impl PathRewrite {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Rejects a prefix that does not start with `/` or that contains `?` or `#`.
    pub fn validate(&self) -> Result<(), Error> {
        let prefix = &self.add_prefix;
        if prefix.is_empty() || (prefix.starts_with('/') && !prefix.contains(['?', '#'])) {
            Ok(())
        } else {
            Err(Error::InvalidPathPrefix {
                prefix: prefix.clone(),
            })
        }
    }
}

fn default_strip_prefix() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

/// A regular expression over a request path.
#[derive(Debug, Clone)]
pub struct PathRegex(Regex);

impl PathRegex {
    /// Replaces the first match in the path. A path without a match stays unchanged.
    pub fn replace<'a>(&self, path: &'a str, replacement: &str) -> Cow<'a, str> {
        self.0.replace(path, replacement)
    }

    /// Returns the template with `${1}` or `${name}` replaced by the capture groups of the first
    /// match, or `None` when the text does not match.
    pub fn expand(&self, text: &str, template: &str) -> Option<String> {
        let captures = self.0.captures(text)?;
        let mut expanded = String::new();
        captures.expand(template, &mut expanded);
        Some(expanded)
    }
}

impl PartialEq for PathRegex {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

impl Eq for PathRegex {}

impl fmt::Display for PathRegex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl FromStr for PathRegex {
    type Err = Error;

    fn from_str(pattern: &str) -> Result<Self, Self::Err> {
        Regex::new(pattern)
            .map(Self)
            .map_err(|_| Error::InvalidPathRegex {
                pattern: pattern.into(),
            })
    }
}

impl Serialize for PathRegex {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for PathRegex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let pattern = String::deserialize(deserializer)?;
        Self::from_str(&pattern).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_rewrite_round_trips_and_rejects_invalid_values() {
        let value = json!({
            "strip_prefix": false,
            "regex": "^/items/([0-9]+)$",
            "replacement": "/item/${1}",
            "add_prefix": "/v2",
        });
        let rewrite: PathRewrite = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&rewrite).unwrap(), value);
        assert!(rewrite.validate().is_ok());
        let regex = rewrite.regex.as_ref().unwrap();
        assert_eq!(regex.replace("/items/42", &rewrite.replacement), "/item/42");
        assert_eq!(regex.replace("/other", &rewrite.replacement), "/other");

        assert_eq!(
            serde_json::to_value(PathRewrite::default()).unwrap(),
            json!({})
        );
        assert!(serde_json::from_value::<PathRewrite>(json!({ "regex": "(" })).is_err());
        assert!(matches!(
            "(".parse::<PathRegex>(),
            Err(Error::InvalidPathRegex { pattern }) if pattern == "("
        ));

        for prefix in ["v2", "/v2?a=1", "/v2#top"] {
            let rewrite = PathRewrite {
                add_prefix: prefix.into(),
                ..Default::default()
            };
            assert!(
                matches!(rewrite.validate(), Err(Error::InvalidPathPrefix { .. })),
                "{prefix}"
            );
        }
    }
}
