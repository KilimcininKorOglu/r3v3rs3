use crate::compression_dictionary::CompressionDictionary;
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

    /// Automatically adjust compression quality based on response size and content type.
    #[serde(default = "default_adaptive_level")]
    pub adaptive_level: bool,

    /// Serve pre-compressed static assets (.br, .zst, .gz) directly without CPU overhead.
    #[serde(default = "default_precompressed")]
    pub precompressed: bool,

    /// Pre-shared compression dictionaries (RFC 9651 / RFC 9652) served and used by this proxy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dictionaries: Vec<CompressionDictionary>,
}

pub const DEFAULT_MIN_SIZE: u64 = 1024;

fn default_adaptive_level() -> bool {
    true
}

fn default_precompressed() -> bool {
    true
}

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
        self.dictionaries
            .iter()
            .try_for_each(CompressionDictionary::validate)?;
        match self.mime_types.iter().find(|mime| !is_mime_pattern(mime)) {
            Some(mime) => Err(Error::InvalidMimeType { mime: mime.clone() }),
            None => Ok(()),
        }
    }

    /// Returns true when the media type of a `Content-Type` value is in `mime_types`
    /// and not an inherently pre-compressed format.
    pub fn compresses_type(&self, content_type: &str) -> bool {
        let essence = content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if is_incompressible_type(&essence) {
            return false;
        }
        let Some((kind, _)) = essence.split_once('/') else {
            return false;
        };
        self.mime_types.iter().any(|pattern| {
            let pattern = pattern.to_ascii_lowercase();
            pattern == essence || pattern.strip_suffix("/*") == Some(kind)
        })
    }
}

const INCOMPRESSIBLE_IMAGE_SUBTYPES: &[&str] = &[
    "png", "jpeg", "jpg", "webp", "avif", "gif", "jxl", "heic", "heif",
];

const INCOMPRESSIBLE_APP_TYPES: &[&str] = &[
    "application/zip",
    "application/gzip",
    "application/x-gzip",
    "application/zstd",
    "application/x-brotli",
    "application/x-tar",
    "application/x-7z-compressed",
    "application/x-rar-compressed",
    "application/pdf",
    "application/vnd.rar",
];

const INCOMPRESSIBLE_FONTS: &[&str] = &[
    "font/woff",
    "font/woff2",
    "application/font-woff",
    "application/font-woff2",
    "application/x-font-woff",
];

/// Returns true when a MIME media type represents an already-compressed binary format
/// that should not be dynamically re-compressed.
pub fn is_incompressible_type(mime: &str) -> bool {
    let essence = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    let Some((kind, subtype)) = essence.split_once('/') else {
        return false;
    };
    if kind == "audio" || kind == "video" {
        return true;
    }
    if kind == "image" && INCOMPRESSIBLE_IMAGE_SUBTYPES.contains(&subtype) {
        return true;
    }
    if INCOMPRESSIBLE_APP_TYPES.contains(&essence.as_str()) {
        return true;
    }
    INCOMPRESSIBLE_FONTS.contains(&essence.as_str())
}

/// Computes an adaptive minimum content size threshold tailored to the given MIME type.
pub fn smart_min_size_for_mime(mime: &str, base_min_size: u64) -> u64 {
    let essence = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    if essence.starts_with("text/")
        || essence == "application/json"
        || essence == "application/javascript"
        || essence == "image/svg+xml"
        || essence == "application/xml"
    {
        base_min_size.min(256)
    } else if essence == "application/wasm" {
        base_min_size.max(2048)
    } else {
        base_min_size
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
    #[serde(rename = "dcb")]
    Dcb,
    #[serde(rename = "dcz")]
    Dcz,
}

impl CompressionAlgorithm {
    pub const ALL: [CompressionAlgorithm; 5] =
        [Self::Brotli, Self::Zstd, Self::Gzip, Self::Dcb, Self::Dcz];

    /// The content coding name in `Accept-Encoding` and `Content-Encoding`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Zstd => "zstd",
            Self::Gzip => "gzip",
            Self::Dcb => "dcb",
            Self::Dcz => "dcz",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Brotli => "Brotli",
            Self::Zstd => "Zstandard",
            Self::Gzip => "Gzip",
            Self::Dcb => "Brotli Dictionary (dcb)",
            Self::Dcz => "Zstandard Dictionary (dcz)",
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

    #[test]
    fn incompressible_types_are_detected() {
        assert!(is_incompressible_type("image/png"));
        assert!(is_incompressible_type("image/jpeg"));
        assert!(is_incompressible_type("image/webp"));
        assert!(is_incompressible_type("image/avif"));
        assert!(is_incompressible_type("video/mp4"));
        assert!(is_incompressible_type("audio/mpeg"));
        assert!(is_incompressible_type("application/zip"));
        assert!(is_incompressible_type("font/woff2"));
        assert!(!is_incompressible_type("text/html"));
        assert!(!is_incompressible_type("application/json"));
        assert!(!is_incompressible_type("image/svg+xml"));
    }

    #[test]
    fn smart_min_size_adapts_to_mime_type() {
        assert_eq!(smart_min_size_for_mime("text/html", 1024), 256);
        assert_eq!(smart_min_size_for_mime("application/json", 1024), 256);
        assert_eq!(smart_min_size_for_mime("application/wasm", 1024), 2048);
        assert_eq!(
            smart_min_size_for_mime("application/octet-stream", 1024),
            1024
        );
    }
}
