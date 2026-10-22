//! DNS CNAME record lookup and alias verification for custom domains.

use anyhow::Result;
use hickory_proto::rr::{RData, RecordType};
use hickory_resolver::{Resolver, TokioResolver, lookup::Lookup, system_conf::read_system_conf};
use r3v3rs3_api::cert_coverage::CnameVerifyResult;
use std::time::Duration;
use tokio::time::timeout;

const DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

fn build_resolver() -> Result<TokioResolver> {
    let (config, opts) = read_system_conf().unwrap_or_default();
    let mut builder = Resolver::builder_with_config(
        config,
        hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
    );
    *builder.options_mut() = opts;
    Ok(builder.build()?)
}

fn validate_inputs(domain: &str, target: &str) -> Option<CnameVerifyResult> {
    if domain.is_empty() || target.is_empty() {
        return Some(CnameVerifyResult {
            domain: domain.to_string(),
            expected_target: target.to_string(),
            resolved_cname: None,
            is_valid: false,
            message: "Domain and expected target cannot be empty".to_string(),
        });
    }
    if domain.eq_ignore_ascii_case(target) {
        return Some(CnameVerifyResult {
            domain: domain.to_string(),
            expected_target: target.to_string(),
            resolved_cname: Some(domain.to_string()),
            is_valid: true,
            message: "Domain matches target host directly".to_string(),
        });
    }
    None
}

fn extract_cname_target(lookup: &Lookup) -> Option<String> {
    lookup
        .answers()
        .iter()
        .find_map(|record| match &record.data {
            RData::CNAME(cname) => {
                Some(cname.to_string().trim_end_matches('.').to_ascii_lowercase())
            }
            _ => None,
        })
}

fn evaluate_cname(domain: String, target: String, cname: Option<String>) -> CnameVerifyResult {
    match cname {
        Some(cname) => {
            let is_valid = cname == target || cname.ends_with(&format!(".{target}"));
            let message = if is_valid {
                format!("CNAME correctly points to {cname}")
            } else {
                format!("CNAME points to {cname}, but expected {target}")
            };
            CnameVerifyResult {
                domain,
                expected_target: target,
                resolved_cname: Some(cname),
                is_valid,
                message,
            }
        }
        None => CnameVerifyResult {
            domain,
            expected_target: target,
            resolved_cname: None,
            is_valid: false,
            message: "No CNAME record found for domain".to_string(),
        },
    }
}

/// Resolves the CNAME record for a domain, comparing it against the expected target.
pub async fn verify_domain_cname(
    domain: &str,
    expected_target: Option<&str>,
    fallback_target: &str,
) -> CnameVerifyResult {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let target = expected_target
        .map(|t| t.trim().trim_end_matches('.').to_ascii_lowercase())
        .unwrap_or_else(|| {
            fallback_target
                .trim()
                .trim_end_matches('.')
                .to_ascii_lowercase()
        });

    if let Some(early_result) = validate_inputs(&domain, &target) {
        return early_result;
    }

    let resolver = match build_resolver() {
        Ok(res) => res,
        Err(err) => {
            return CnameVerifyResult {
                domain,
                expected_target: target,
                resolved_cname: None,
                is_valid: false,
                message: format!("Failed to initialize DNS resolver: {err}"),
            };
        }
    };

    let fqdn = format!("{domain}.");
    let lookup_result = timeout(DNS_LOOKUP_TIMEOUT, resolver.lookup(fqdn, RecordType::CNAME)).await;

    match lookup_result {
        Ok(Ok(lookup)) => {
            let found_cname = extract_cname_target(&lookup);
            evaluate_cname(domain, target, found_cname)
        }
        Ok(Err(err)) => CnameVerifyResult {
            domain,
            expected_target: target,
            resolved_cname: None,
            is_valid: false,
            message: format!("DNS CNAME lookup returned error: {err}"),
        },
        Err(_) => CnameVerifyResult {
            domain,
            expected_target: target,
            resolved_cname: None,
            is_valid: false,
            message: "DNS lookup timed out".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_inputs_empty() {
        let res = validate_inputs("", "example.com").unwrap();
        assert!(!res.is_valid);

        let res = validate_inputs("app.example.com", "").unwrap();
        assert!(!res.is_valid);
    }

    #[test]
    fn test_validate_inputs_identical() {
        let res = validate_inputs("example.com", "example.com").unwrap();
        assert!(res.is_valid);
        assert_eq!(res.resolved_cname, Some("example.com".to_string()));
    }

    #[test]
    fn test_evaluate_cname_matching() {
        let res = evaluate_cname(
            "portal.client.com".into(),
            "app.example.com".into(),
            Some("app.example.com".into()),
        );
        assert!(res.is_valid);
        assert_eq!(res.resolved_cname, Some("app.example.com".into()));
    }

    #[test]
    fn test_evaluate_cname_subdomain_matching() {
        let res = evaluate_cname(
            "portal.client.com".into(),
            "example.com".into(),
            Some("app.example.com".into()),
        );
        assert!(res.is_valid);
    }

    #[test]
    fn test_evaluate_cname_mismatch() {
        let res = evaluate_cname(
            "portal.client.com".into(),
            "app.example.com".into(),
            Some("other.domain.com".into()),
        );
        assert!(!res.is_valid);
    }

    #[test]
    fn test_evaluate_cname_none() {
        let res = evaluate_cname("portal.client.com".into(), "app.example.com".into(), None);
        assert!(!res.is_valid);
        assert_eq!(res.resolved_cname, None);
    }
}
