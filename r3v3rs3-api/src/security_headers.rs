use crate::header_rules::HeaderRule;
use serde_derive::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Predefined OWASP security header hardening profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecurityProfile {
    /// Highest security level: Preloaded HSTS, strict CSP, no-referrer, deny framing.
    Strict,
    /// Standard web profile: 1-year HSTS, balanced CSP, sameorigin framing.
    Balanced,
    /// API profile: HSTS, no-sniff, deny framing, no-store cache, minimal CSP.
    ApiOnly,
    /// Modern Single-Page Application profile: HSTS, SPA-optimized CSP and workers.
    SpaFrontend,
}

impl SecurityProfile {
    /// Returns all available predefined security profiles.
    pub fn all() -> &'static [SecurityProfile] {
        &[
            SecurityProfile::Strict,
            SecurityProfile::Balanced,
            SecurityProfile::ApiOnly,
            SecurityProfile::SpaFrontend,
        ]
    }

    /// Returns the human-readable name of the profile.
    pub fn title(&self) -> &'static str {
        match self {
            Self::Strict => "Strict (OWASP High)",
            Self::Balanced => "Balanced (Standard Web)",
            Self::ApiOnly => "API Only (REST/JSON)",
            Self::SpaFrontend => "SPA / Modern Web App",
        }
    }

    /// Returns a short description of what this profile hardens.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Strict => {
                "Maximum defense with preloaded HSTS, strict script CSP, and complete clickjacking lockdown."
            }
            Self::Balanced => {
                "Production-grade OWASP defaults compatible with content-rich sites, CMSs, and standard web apps."
            }
            Self::ApiOnly => {
                "Optimized for headless APIs and microservices with anti-sniffing, framing denial, and cache shields."
            }
            Self::SpaFrontend => {
                "Tailored for React, Vue, Vite, and PWA apps with WebSocket, blob, and worker-friendly directives."
            }
        }
    }

    /// Generates response header rules corresponding to this profile.
    pub fn generate_rules(&self) -> Vec<HeaderRule> {
        match self {
            Self::Strict => generate_strict_rules(),
            Self::Balanced => generate_balanced_rules(),
            Self::ApiOnly => generate_api_only_rules(),
            Self::SpaFrontend => generate_spa_rules(),
        }
    }

    /// Converts into a profile information descriptor.
    pub fn to_info(&self) -> SecurityProfileInfo {
        SecurityProfileInfo {
            id: *self,
            title: self.title().to_string(),
            description: self.description().to_string(),
            rules: self.generate_rules(),
        }
    }
}

fn generate_strict_rules() -> Vec<HeaderRule> {
    vec![
        HeaderRule::Set {
            name: "Strict-Transport-Security".into(),
            value: "max-age=63072000; includeSubDomains; preload".into(),
        },
        HeaderRule::Set {
            name: "X-Content-Type-Options".into(),
            value: "nosniff".into(),
        },
        HeaderRule::Set {
            name: "X-Frame-Options".into(),
            value: "DENY".into(),
        },
        HeaderRule::Set {
            name: "Referrer-Policy".into(),
            value: "no-referrer".into(),
        },
        HeaderRule::Set {
            name: "Permissions-Policy".into(),
            value: "accelerometer=(), camera=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), payment=(), usb=()".into(),
        },
        HeaderRule::Set {
            name: "Content-Security-Policy".into(),
            value: "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'none'; form-action 'self'".into(),
        },
        HeaderRule::Set {
            name: "Cross-Origin-Opener-Policy".into(),
            value: "same-origin".into(),
        },
        HeaderRule::Set {
            name: "Cross-Origin-Resource-Policy".into(),
            value: "same-origin".into(),
        },
    ]
}

fn generate_balanced_rules() -> Vec<HeaderRule> {
    vec![
        HeaderRule::Set {
            name: "Strict-Transport-Security".into(),
            value: "max-age=31536000; includeSubDomains".into(),
        },
        HeaderRule::Set {
            name: "X-Content-Type-Options".into(),
            value: "nosniff".into(),
        },
        HeaderRule::Set {
            name: "X-Frame-Options".into(),
            value: "SAMEORIGIN".into(),
        },
        HeaderRule::Set {
            name: "Referrer-Policy".into(),
            value: "strict-origin-when-cross-origin".into(),
        },
        HeaderRule::Set {
            name: "Permissions-Policy".into(),
            value: "camera=(), microphone=(), geolocation=()".into(),
        },
        HeaderRule::Set {
            name: "Content-Security-Policy".into(),
            value: "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self' data:; connect-src 'self'; frame-ancestors 'self'".into(),
        },
    ]
}

fn generate_api_only_rules() -> Vec<HeaderRule> {
    vec![
        HeaderRule::Set {
            name: "Strict-Transport-Security".into(),
            value: "max-age=31536000; includeSubDomains".into(),
        },
        HeaderRule::Set {
            name: "X-Content-Type-Options".into(),
            value: "nosniff".into(),
        },
        HeaderRule::Set {
            name: "X-Frame-Options".into(),
            value: "DENY".into(),
        },
        HeaderRule::Set {
            name: "Referrer-Policy".into(),
            value: "no-referrer".into(),
        },
        HeaderRule::Set {
            name: "Cache-Control".into(),
            value: "no-store, max-age=0".into(),
        },
        HeaderRule::Set {
            name: "Content-Security-Policy".into(),
            value: "default-src 'none'; frame-ancestors 'none'".into(),
        },
    ]
}

fn generate_spa_rules() -> Vec<HeaderRule> {
    vec![
        HeaderRule::Set {
            name: "Strict-Transport-Security".into(),
            value: "max-age=31536000; includeSubDomains".into(),
        },
        HeaderRule::Set {
            name: "X-Content-Type-Options".into(),
            value: "nosniff".into(),
        },
        HeaderRule::Set {
            name: "X-Frame-Options".into(),
            value: "SAMEORIGIN".into(),
        },
        HeaderRule::Set {
            name: "Referrer-Policy".into(),
            value: "strict-origin-when-cross-origin".into(),
        },
        HeaderRule::Set {
            name: "Content-Security-Policy".into(),
            value: "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self' wss: https:; worker-src 'self' blob:; frame-ancestors 'self'".into(),
        },
    ]
}

/// Information descriptor for a security profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SecurityProfileInfo {
    pub id: SecurityProfile,
    pub title: String,
    pub description: String,
    pub rules: Vec<HeaderRule>,
}

/// Request payload to apply a predefined security profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApplySecurityProfileRequest {
    pub profile: SecurityProfile,
}

/// Request payload to analyze an arbitrary list of response header rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AnalyzeHeadersRequest {
    pub rules: Vec<HeaderRule>,
}

/// Calculated security grade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecurityGrade {
    APlus,
    A,
    B,
    C,
    D,
    F,
}

impl SecurityGrade {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::APlus => "A+",
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::F => "F",
        }
    }
}

/// Status of an individual security header audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HeaderAuditStatus {
    Present,
    Weak,
    Missing,
}

/// Audit details for a single security header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HeaderAuditItem {
    pub header_name: String,
    pub status: HeaderAuditStatus,
    pub current_value: Option<String>,
    pub recommended_value: String,
    pub risk: String,
    pub score_weight: u8,
    pub earned_score: u8,
}

/// Comprehensive audit report of security response headers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SecurityHeadersAudit {
    pub score: u8,
    pub grade: SecurityGrade,
    pub items: Vec<HeaderAuditItem>,
    pub detected_profile: Option<SecurityProfile>,
}

/// Applies a security profile's rules to an existing list of response rules,
/// replacing or inserting corresponding security headers cleanly.
pub fn apply_profile_to_rules(
    existing: &[HeaderRule],
    profile: SecurityProfile,
) -> Vec<HeaderRule> {
    let new_rules = profile.generate_rules();
    let mut result: Vec<HeaderRule> = existing.to_vec();

    for rule in new_rules {
        let name = rule.name();
        let pos = result
            .iter()
            .position(|r| r.name().eq_ignore_ascii_case(name));
        match pos {
            Some(idx) => {
                result[idx] = rule;
            }
            None => {
                result.push(rule);
            }
        }
    }

    result
}

/// Analyzes a set of response header rules and produces a security score and audit breakdown.
pub fn analyze_response_rules(rules: &[HeaderRule]) -> SecurityHeadersAudit {
    let items = vec![
        audit_hsts(rules),
        audit_csp(rules),
        audit_x_frame_options(rules),
        audit_x_content_type_options(rules),
        audit_referrer_policy(rules),
        audit_permissions_policy(rules),
    ];

    let score: u8 = items.iter().map(|i| i.earned_score).sum();
    let grade = calculate_grade(score);
    let detected_profile = detect_matching_profile(rules);

    SecurityHeadersAudit {
        score,
        grade,
        items,
        detected_profile,
    }
}

fn calculate_grade(score: u8) -> SecurityGrade {
    if score >= 95 {
        SecurityGrade::APlus
    } else if score >= 80 {
        SecurityGrade::A
    } else if score >= 65 {
        SecurityGrade::B
    } else if score >= 50 {
        SecurityGrade::C
    } else if score >= 35 {
        SecurityGrade::D
    } else {
        SecurityGrade::F
    }
}

fn find_header_value<'a>(rules: &'a [HeaderRule], name: &str) -> Option<&'a str> {
    rules.iter().rev().find_map(|r| match r {
        HeaderRule::Set {
            name: rname, value, ..
        } if rname.eq_ignore_ascii_case(name) => Some(value.as_str()),
        HeaderRule::Append {
            name: rname, value, ..
        } if rname.eq_ignore_ascii_case(name) => Some(value.as_str()),
        HeaderRule::Remove { name: rname } if rname.eq_ignore_ascii_case(name) => None,
        _ => None,
    })
}

fn audit_hsts(rules: &[HeaderRule]) -> HeaderAuditItem {
    let header_name = "Strict-Transport-Security".to_string();
    let weight = 25;
    let rec = "max-age=31536000; includeSubDomains".to_string();
    let risk = "Man-in-the-Middle (MITM) and SSL Stripping attacks".to_string();

    match find_header_value(rules, &header_name) {
        Some(val) => {
            let lower = val.to_ascii_lowercase();
            let has_sub = lower.contains("includesubdomains");
            let has_max = lower.contains("max-age=");
            if has_max && has_sub {
                HeaderAuditItem {
                    header_name,
                    status: HeaderAuditStatus::Present,
                    current_value: Some(val.to_string()),
                    recommended_value: rec,
                    risk,
                    score_weight: weight,
                    earned_score: weight,
                }
            } else {
                HeaderAuditItem {
                    header_name,
                    status: HeaderAuditStatus::Weak,
                    current_value: Some(val.to_string()),
                    recommended_value: rec,
                    risk,
                    score_weight: weight,
                    earned_score: 15,
                }
            }
        }
        None => HeaderAuditItem {
            header_name,
            status: HeaderAuditStatus::Missing,
            current_value: None,
            recommended_value: rec,
            risk,
            score_weight: weight,
            earned_score: 0,
        },
    }
}

fn audit_csp(rules: &[HeaderRule]) -> HeaderAuditItem {
    let header_name = "Content-Security-Policy".to_string();
    let weight = 25;
    let rec = "default-src 'self'; frame-ancestors 'self'".to_string();
    let risk = "Cross-Site Scripting (XSS) and code injection vulnerabilities".to_string();

    match find_header_value(rules, &header_name) {
        Some(val) => {
            let lower = val.to_ascii_lowercase();
            let is_strong = lower.contains("default-src") || lower.contains("script-src");
            if is_strong {
                HeaderAuditItem {
                    header_name,
                    status: HeaderAuditStatus::Present,
                    current_value: Some(val.to_string()),
                    recommended_value: rec,
                    risk,
                    score_weight: weight,
                    earned_score: weight,
                }
            } else {
                HeaderAuditItem {
                    header_name,
                    status: HeaderAuditStatus::Weak,
                    current_value: Some(val.to_string()),
                    recommended_value: rec,
                    risk,
                    score_weight: weight,
                    earned_score: 15,
                }
            }
        }
        None => HeaderAuditItem {
            header_name,
            status: HeaderAuditStatus::Missing,
            current_value: None,
            recommended_value: rec,
            risk,
            score_weight: weight,
            earned_score: 0,
        },
    }
}

fn audit_x_frame_options(rules: &[HeaderRule]) -> HeaderAuditItem {
    let header_name = "X-Frame-Options".to_string();
    let weight = 15;
    let rec = "SAMEORIGIN".to_string();
    let risk = "Clickjacking and UI redress attacks".to_string();

    match find_header_value(rules, &header_name) {
        Some(val) => {
            let upper = val.to_ascii_uppercase();
            if upper.contains("DENY") || upper.contains("SAMEORIGIN") {
                HeaderAuditItem {
                    header_name,
                    status: HeaderAuditStatus::Present,
                    current_value: Some(val.to_string()),
                    recommended_value: rec,
                    risk,
                    score_weight: weight,
                    earned_score: weight,
                }
            } else {
                HeaderAuditItem {
                    header_name,
                    status: HeaderAuditStatus::Weak,
                    current_value: Some(val.to_string()),
                    recommended_value: rec,
                    risk,
                    score_weight: weight,
                    earned_score: 8,
                }
            }
        }
        None => HeaderAuditItem {
            header_name,
            status: HeaderAuditStatus::Missing,
            current_value: None,
            recommended_value: rec,
            risk,
            score_weight: weight,
            earned_score: 0,
        },
    }
}

fn audit_x_content_type_options(rules: &[HeaderRule]) -> HeaderAuditItem {
    let header_name = "X-Content-Type-Options".to_string();
    let weight = 15;
    let rec = "nosniff".to_string();
    let risk = "MIME-type confusion and drive-by download exploits".to_string();

    match find_header_value(rules, &header_name) {
        Some(val) => {
            if val.eq_ignore_ascii_case("nosniff") {
                HeaderAuditItem {
                    header_name,
                    status: HeaderAuditStatus::Present,
                    current_value: Some(val.to_string()),
                    recommended_value: rec,
                    risk,
                    score_weight: weight,
                    earned_score: weight,
                }
            } else {
                HeaderAuditItem {
                    header_name,
                    status: HeaderAuditStatus::Weak,
                    current_value: Some(val.to_string()),
                    recommended_value: rec,
                    risk,
                    score_weight: weight,
                    earned_score: 5,
                }
            }
        }
        None => HeaderAuditItem {
            header_name,
            status: HeaderAuditStatus::Missing,
            current_value: None,
            recommended_value: rec,
            risk,
            score_weight: weight,
            earned_score: 0,
        },
    }
}

fn audit_referrer_policy(rules: &[HeaderRule]) -> HeaderAuditItem {
    let header_name = "Referrer-Policy".to_string();
    let weight = 10;
    let rec = "strict-origin-when-cross-origin".to_string();
    let risk = "Sensitive path or token leakage via Referer header".to_string();

    match find_header_value(rules, &header_name) {
        Some(val) => {
            let lower = val.to_ascii_lowercase();
            let is_good = lower.contains("strict-origin") || lower.contains("no-referrer");
            if is_good {
                HeaderAuditItem {
                    header_name,
                    status: HeaderAuditStatus::Present,
                    current_value: Some(val.to_string()),
                    recommended_value: rec,
                    risk,
                    score_weight: weight,
                    earned_score: weight,
                }
            } else {
                HeaderAuditItem {
                    header_name,
                    status: HeaderAuditStatus::Weak,
                    current_value: Some(val.to_string()),
                    recommended_value: rec,
                    risk,
                    score_weight: weight,
                    earned_score: 5,
                }
            }
        }
        None => HeaderAuditItem {
            header_name,
            status: HeaderAuditStatus::Missing,
            current_value: None,
            recommended_value: rec,
            risk,
            score_weight: weight,
            earned_score: 0,
        },
    }
}

fn audit_permissions_policy(rules: &[HeaderRule]) -> HeaderAuditItem {
    let header_name = "Permissions-Policy".to_string();
    let weight = 10;
    let rec = "camera=(), microphone=(), geolocation=()".to_string();
    let risk = "Unrestricted client hardware and biometric sensor access".to_string();

    match find_header_value(rules, &header_name) {
        Some(val) => HeaderAuditItem {
            header_name,
            status: HeaderAuditStatus::Present,
            current_value: Some(val.to_string()),
            recommended_value: rec,
            risk,
            score_weight: weight,
            earned_score: weight,
        },
        None => HeaderAuditItem {
            header_name,
            status: HeaderAuditStatus::Missing,
            current_value: None,
            recommended_value: rec,
            risk,
            score_weight: weight,
            earned_score: 0,
        },
    }
}

fn detect_matching_profile(rules: &[HeaderRule]) -> Option<SecurityProfile> {
    for profile in SecurityProfile::all() {
        if matches_profile(rules, *profile) {
            return Some(*profile);
        }
    }
    None
}

fn matches_profile(rules: &[HeaderRule], profile: SecurityProfile) -> bool {
    let prof_rules = profile.generate_rules();
    for rule in &prof_rules {
        let name = rule.name();
        let expected = match rule {
            HeaderRule::Set { value, .. } => value.as_str(),
            _ => continue,
        };
        match find_header_value(rules, name) {
            Some(v) if v.eq_ignore_ascii_case(expected) => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_rules_analysis() {
        let audit = analyze_response_rules(&[]);
        assert_eq!(audit.score, 0);
        assert_eq!(audit.grade, SecurityGrade::F);
        assert_eq!(audit.items.len(), 6);
        assert!(audit.detected_profile.is_none());
    }

    #[test]
    fn test_strict_profile_application_and_score() {
        let applied = apply_profile_to_rules(&[], SecurityProfile::Strict);
        assert!(!applied.is_empty());
        let audit = analyze_response_rules(&applied);
        assert_eq!(audit.score, 100);
        assert_eq!(audit.grade, SecurityGrade::APlus);
        assert_eq!(audit.detected_profile, Some(SecurityProfile::Strict));
    }

    #[test]
    fn test_balanced_profile_application_and_score() {
        let applied = apply_profile_to_rules(&[], SecurityProfile::Balanced);
        let audit = analyze_response_rules(&applied);
        assert_eq!(audit.score, 100);
        assert_eq!(audit.grade, SecurityGrade::APlus);
        assert_eq!(audit.detected_profile, Some(SecurityProfile::Balanced));
    }

    #[test]
    fn test_merge_with_existing_custom_headers() {
        let existing = vec![
            HeaderRule::Set {
                name: "X-Custom-App".into(),
                value: "1.0".into(),
            },
            HeaderRule::Set {
                name: "X-Frame-Options".into(),
                value: "ALLOWALL".into(),
            },
        ];
        let applied = apply_profile_to_rules(&existing, SecurityProfile::Strict);
        // X-Custom-App must be preserved
        assert!(applied.iter().any(|r| r.name() == "X-Custom-App"));
        // X-Frame-Options must be overwritten to DENY
        let frame_rule = applied
            .iter()
            .find(|r| r.name().eq_ignore_ascii_case("X-Frame-Options"))
            .unwrap();
        if let HeaderRule::Set { value, .. } = frame_rule {
            assert_eq!(value, "DENY");
        } else {
            panic!("Expected Set rule");
        }
    }
}
