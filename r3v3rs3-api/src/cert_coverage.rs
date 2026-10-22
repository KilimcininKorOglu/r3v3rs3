use crate::id::ShortId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The SSL/TLS coverage status of a given domain name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainSslStatus {
    /// Covered by an active wildcard certificate (e.g. `*.example.com`) with zero manual configuration.
    CoveredByWildcard {
        wildcard_cert_id: ShortId,
        pattern: String,
    },
    /// Covered by an exact dedicated certificate.
    DedicatedCert { cert_id: ShortId },
    /// An external custom domain pointing via CNAME or A record, requiring DNS aliasing.
    CnameOrExternal { suggested_cname_target: String },
    /// A matching certificate was found, but it has expired.
    Expired { cert_id: ShortId },
    /// No certificate currently matches this domain.
    NoCertificate,
}

/// Detailed certificate coverage report for a domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DomainCoverageInfo {
    pub domain: String,
    pub status: DomainSslStatus,
    pub is_wildcard_shared: bool,
    pub root_domain: Option<String>,
}

impl DomainCoverageInfo {
    pub fn new(domain: impl Into<String>, status: DomainSslStatus) -> Self {
        let domain = domain.into();
        let is_wildcard_shared = matches!(status, DomainSslStatus::CoveredByWildcard { .. });
        let root_domain = extract_root_domain(&domain);
        Self {
            domain,
            status,
            is_wildcard_shared,
            root_domain,
        }
    }
}

/// Request to verify a domain's CNAME record in public DNS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CnameVerifyRequest {
    pub domain: String,
    pub expected_target: Option<String>,
}

/// Result of a CNAME DNS verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CnameVerifyResult {
    pub domain: String,
    pub expected_target: String,
    pub resolved_cname: Option<String>,
    pub is_valid: bool,
    pub message: String,
}

/// Extracts the parent/root domain pattern from a subdomain.
///
/// E.g. `app.example.com` -> `Some("example.com")`
/// E.g. `foo.bar.example.com` -> `Some("bar.example.com")`
/// E.g. `example.com` -> `None`
pub fn extract_root_domain(domain: &str) -> Option<String> {
    let trimmed = domain.trim().trim_end_matches('.');
    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() >= 3 {
        Some(parts[1..].join("."))
    } else {
        None
    }
}

/// Returns the wildcard pattern for a given domain if it has a root domain.
///
/// E.g. `app.example.com` -> `Some("*.example.com")`
pub fn wildcard_pattern_for(domain: &str) -> Option<String> {
    extract_root_domain(domain).map(|root| format!("*.{root}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_root_domain() {
        assert_eq!(
            extract_root_domain("app.example.com"),
            Some("example.com".to_string())
        );
        assert_eq!(
            extract_root_domain("foo.bar.example.com"),
            Some("bar.example.com".to_string())
        );
        assert_eq!(extract_root_domain("example.com"), None);
        assert_eq!(extract_root_domain("localhost"), None);
        assert_eq!(extract_root_domain(""), None);
    }

    #[test]
    fn test_wildcard_pattern_for() {
        assert_eq!(
            wildcard_pattern_for("app.example.com"),
            Some("*.example.com".to_string())
        );
        assert_eq!(wildcard_pattern_for("example.com"), None);
    }

    #[test]
    fn test_domain_coverage_info_creation() {
        let id: ShortId = "a13e1ecc080e42".parse().unwrap();
        let info = DomainCoverageInfo::new(
            "app.example.com",
            DomainSslStatus::CoveredByWildcard {
                wildcard_cert_id: id,
                pattern: "*.example.com".to_string(),
            },
        );
        assert!(info.is_wildcard_shared);
        assert_eq!(info.root_domain, Some("example.com".to_string()));
    }
}
