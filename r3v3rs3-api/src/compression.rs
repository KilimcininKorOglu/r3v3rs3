use crate::error::Error;
use crate::policy::is_header_name;
use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Compresses upstream responses with an encoding that the client accepts.
#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Compression {
    /// Encodings in the order of preference. Empty disables compression.
    #[serde(default)]
    pub algorithms: Vec<CompressionAlgorithm>,

    /// Responses with a smaller `Content-Length` in bytes are not compressed.
    #[serde(default = "default_min_size")]
    pub min_size: u64,

    /// Media types to compress. `text/*` matches every text type.
    #[serde(default = "default_mime_types")]
    pub mime_types: Vec<String>,
}

pub const DEFAULT_MIN_SIZE: u64 = 1024;

pub const DEFAULT_MIME_TYPES: &[&str] = &[
    "text/*",
    "application/javascript",
    "application/json",
    "application/manifest+json",
    "application/wasm",
    "application/xml",
    "application/xhtml+xml",
    "application/rss+xml",
    "application/atom+xml",
    "image/svg+xml",
    "font/otf",
    "font/ttf",
];

fn default_min_size() -> u64 {
    DEFAULT_MIN_SIZE
}

fn default_mime_types() -> Vec<String> {
    DEFAULT_MIME_TYPES
        .iter()
        .map(|mime| mime.to_string())
        .collect()
}

impl Compression {
    pub fn is_disabled(&self) -> bool {
        self.algorithms.is_empty()
    }

    pub fn validate(&self) -> Result<(), Error> {
        for (index, algorithm) in self.algorithms.iter().enumerate() {
            if self.algorithms[..index].contains(algorithm) {
                return Err(Error::DuplicateCompressionAlgorithm {
                    algorithm: algorithm.as_str().into(),
                });
            }
        }
        match self.mime_types.iter().find(|mime| !is_mime_pattern(mime)) {
            Some(mime) => Err(Error::InvalidMimeType { mime: mime.clone() }),
            None => Ok(()),
        }
    }

    /// Returns true when the media type of a `Content-Type` value is in `mime_types`.
    pub fn compresses_type(&self, content_type: &str) -> bool {
        let essence = content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let Some((kind, _)) = essence.split_once('/') else {
            return false;
        };
        self.mime_types.iter().any(|pattern| {
            let pattern = pattern.to_ascii_lowercase();
            pattern == essence || pattern.strip_suffix("/*") == Some(kind)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum CompressionAlgorithm {
    #[serde(rename = "br")]
    Brotli,
    #[serde(rename = "zstd")]
    Zstd,
    #[serde(rename = "gzip")]
    Gzip,
}

impl CompressionAlgorithm {
    pub const ALL: [CompressionAlgorithm; 3] = [Self::Brotli, Self::Zstd, Self::Gzip];

    /// The content coding name in `Accept-Encoding` and `Content-Encoding`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Zstd => "zstd",
            Self::Gzip => "gzip",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Brotli => "Brotli",
            Self::Zstd => "Zstandard",
            Self::Gzip => "Gzip",
        }
    }
}

/// Returns true for `type/subtype` or `type/*`.
fn is_mime_pattern(mime: &str) -> bool {
    match mime.split_once('/') {
        Some((kind, subtype)) => {
            is_header_name(kind)
                && !kind.contains('*')
                && (subtype == "*" || is_header_name(subtype))
        }
        None => false,
    }
}

/// Parses a list of media types separated by commas or whitespace.
pub fn parse_mime_list(text: &str) -> Result<Vec<String>, Error> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|mime| !mime.is_empty())
        .map(|mime| {
            let mime = mime.to_ascii_lowercase();
            if is_mime_pattern(&mime) {
                Ok(mime)
            } else {
                Err(Error::InvalidMimeType { mime })
            }
        })
        .collect()
}

pub fn format_mime_list(mime_types: &[String]) -> String {
    mime_types.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compression(mime_types: &[&str]) -> Compression {
        Compression {
            algorithms: vec![CompressionAlgorithm::Gzip],
            mime_types: mime_types.iter().map(|mime| mime.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn serde_fills_defaults() {
        let config: Compression = serde_json::from_str(r#"{"algorithms":["br","gzip"]}"#).unwrap();
        assert_eq!(
            config.algorithms,
            vec![CompressionAlgorithm::Brotli, CompressionAlgorithm::Gzip]
        );
        assert_eq!(config.min_size, DEFAULT_MIN_SIZE);
        assert_eq!(config.mime_types, default_mime_types());
        assert!(Compression::default().is_disabled());
    }

    #[test]
    fn media_types_match_exactly_or_by_wildcard() {
        let config = compression(&["text/*", "application/json"]);
        assert!(config.compresses_type("text/html; charset=utf-8"));
        assert!(config.compresses_type("Application/JSON"));
        assert!(!config.compresses_type("application/javascript"));
        assert!(!config.compresses_type("image/png"));
        assert!(!config.compresses_type("text"));
    }

    #[test]
    fn invalid_settings_are_rejected() {
        assert!(compression(&["text/*", "image/svg+xml"]).validate().is_ok());
        assert!(matches!(
            compression(&["text"]).validate(),
            Err(Error::InvalidMimeType { .. })
        ));
        assert!(matches!(
            compression(&["*/*"]).validate(),
            Err(Error::InvalidMimeType { .. })
        ));
        let duplicate = Compression {
            algorithms: vec![CompressionAlgorithm::Gzip, CompressionAlgorithm::Gzip],
            ..Default::default()
        };
        assert!(matches!(
            duplicate.validate(),
            Err(Error::DuplicateCompressionAlgorithm { .. })
        ));
    }

    #[test]
    fn mime_list_is_parsed_and_formatted() {
        let list = parse_mime_list("Text/*, application/json\nimage/svg+xml").unwrap();
        assert_eq!(list, vec!["text/*", "application/json", "image/svg+xml"]);
        assert_eq!(
            format_mime_list(&list),
            "text/*, application/json, image/svg+xml"
        );
        assert!(parse_mime_list("text/html, html").is_err());
    }
}
