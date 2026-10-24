use crate::error::Error;
use crate::id::ShortId;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use serde_derive::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

/// A pre-shared compression dictionary (RFC 9651 / RFC 9652) served and used by a reverse proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CompressionDictionary {
    /// Unique identifier of the dictionary.
    pub id: ShortId,

    /// Descriptive human-readable label.
    pub name: String,

    /// URL path match pattern sent in `Use-As-Dictionary: match="<match_pattern>"` (e.g. `/api/*`).
    pub match_pattern: String,

    /// Path on this proxy where r3v3rs3 serves the raw dictionary bytes to clients (e.g. `/.well-known/dict/api.dict` or `/api/dict`).
    pub path: String,

    /// Raw text, JSON schema, or binary dictionary content.
    pub raw_data: String,

    /// RFC 9651 structured field byte sequence representation of the SHA-256 hash: `:<base64>:`.
    #[serde(default)]
    pub sha256_sf: String,
}

impl CompressionDictionary {
    /// Computes the RFC 9651 structured field byte sequence binary string `:<base64>:` for given bytes.
    pub fn calculate_sha256_sf(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let digest = hasher.finalize();
        let b64 = BASE64_STANDARD.encode(digest);
        format!(":{b64}:")
    }

    /// Automatically computes and sets `sha256_sf` if empty.
    pub fn ensure_hash(&mut self) {
        if self.sha256_sf.is_empty() {
            self.sha256_sf = Self::calculate_sha256_sf(self.raw_data.as_bytes());
        }
    }

    /// Validates the dictionary fields.
    pub fn validate(&self) -> Result<(), Error> {
        if self.name.trim().is_empty() {
            return Err(Error::InvalidCompressionDictionary {
                reason: "Dictionary name cannot be empty".to_string(),
            });
        }
        if self.match_pattern.trim().is_empty() {
            return Err(Error::InvalidCompressionDictionary {
                reason: "Match pattern cannot be empty".to_string(),
            });
        }
        if !self.path.starts_with('/') {
            return Err(Error::InvalidCompressionDictionary {
                reason: "Dictionary serving path must start with '/'".to_string(),
            });
        }
        if self.raw_data.is_empty() {
            return Err(Error::InvalidCompressionDictionary {
                reason: "Dictionary raw data cannot be empty".to_string(),
            });
        }
        if self.raw_data.len() > 10 * 1024 * 1024 {
            return Err(Error::InvalidCompressionDictionary {
                reason: "Dictionary raw data cannot exceed 10 MB".to_string(),
            });
        }
        Ok(())
    }

    /// Checks if a request path matches this dictionary's match pattern.
    pub fn matches_path(&self, request_path: &str) -> bool {
        let pattern = self.match_pattern.trim();
        if pattern == "*" || pattern == "/*" {
            return true;
        }
        if let Some(prefix) = pattern.strip_suffix("/*") {
            return request_path == prefix
                || request_path.starts_with(&format!("{prefix}/"))
                || (prefix.is_empty() && request_path.starts_with('/'));
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return request_path.starts_with(prefix);
        }
        request_path == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_sha256_sf() {
        let hash = CompressionDictionary::calculate_sha256_sf(b"hello world");
        assert!(hash.starts_with(':'));
        assert!(hash.ends_with(':'));
        // "hello world" SHA256 base64 is uU0nuZNNPgilLlLX2n2r+sSE7+N6U4DukIj3rOLvzek=
        assert_eq!(hash, ":uU0nuZNNPgilLlLX2n2r+sSE7+N6U4DukIj3rOLvzek=:");
    }

    #[test]
    fn test_validation_rules() {
        let mut dict = CompressionDictionary {
            id: ShortId::new(),
            name: "API Schema".to_string(),
            match_pattern: "/api/*".to_string(),
            path: "/.well-known/dict/api.dict".to_string(),
            raw_data: r#"{"status":200,"data":[]}"#.to_string(),
            sha256_sf: String::new(),
        };
        assert!(dict.validate().is_ok());
        dict.ensure_hash();
        assert!(!dict.sha256_sf.is_empty());

        let invalid_path = CompressionDictionary {
            path: "relative/path".to_string(),
            ..dict.clone()
        };
        assert!(invalid_path.validate().is_err());

        let empty_data = CompressionDictionary {
            raw_data: String::new(),
            ..dict.clone()
        };
        assert!(empty_data.validate().is_err());
    }

    #[test]
    fn test_matches_path() {
        let dict = CompressionDictionary {
            id: ShortId::new(),
            name: "API Schema".to_string(),
            match_pattern: "/api/*".to_string(),
            path: "/.well-known/dict/api.dict".to_string(),
            raw_data: "data".to_string(),
            sha256_sf: String::new(),
        };
        assert!(dict.matches_path("/api/users"));
        assert!(dict.matches_path("/api/products/123"));
        assert!(!dict.matches_path("/static/app.js"));
    }
}
