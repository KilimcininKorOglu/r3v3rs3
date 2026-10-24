use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::server::rpc::proxies::{GetProxy, UpdateProxy};
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::proxy::ProxyKind;
use r3v3rs3_api::security_headers::{
    AnalyzeHeadersRequest, ApplySecurityProfileRequest, SecurityHeadersAudit, SecurityProfile,
    SecurityProfileInfo, analyze_response_rules, apply_profile_to_rules,
};

/// Lists all available predefined OWASP security hardening profiles.
#[utoipa::path(
    get,
    path = "/profiles",
    tag = "security_headers",
    operation_id = "list_security_header_profiles",
    responses((status = 200, description = "The security profiles.", body = Vec<SecurityProfileInfo>), ErrorResponses)
)]
pub async fn list_profiles(
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<SecurityProfileInfo>>, AppError> {
    caller.authorize(Permission::Read)?;
    let profiles = SecurityProfile::all()
        .iter()
        .map(SecurityProfile::to_info)
        .collect();
    Ok(Json(profiles))
}

/// Analyzes an arbitrary list of response header rules and calculates the security audit score.
#[utoipa::path(
    post,
    path = "/analyze",
    tag = "security_headers",
    operation_id = "analyze_security_headers",
    request_body = AnalyzeHeadersRequest,
    responses((status = 200, description = "The security audit report.", body = SecurityHeadersAudit), ErrorResponses)
)]
pub async fn analyze_headers(
    Extension(caller): Extension<Caller>,
    Json(body): Json<AnalyzeHeadersRequest>,
) -> Result<Json<SecurityHeadersAudit>, AppError> {
    caller.authorize(Permission::Read)?;
    let audit = analyze_response_rules(&body.rules);
    Ok(Json(audit))
}

/// Returns the security audit report for an existing HTTP proxy.
#[utoipa::path(
    get,
    path = "/{id}/security-headers",
    tag = "security_headers",
    operation_id = "get_proxy_security_headers",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    responses((status = 200, description = "The proxy security audit.", body = SecurityHeadersAudit), NotFoundResponse, ErrorResponses)
)]
pub async fn get_proxy_security_headers(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<SecurityHeadersAudit>, AppError> {
    let entry = *state.call(&caller, GetProxy { id }).await?;
    match &entry.proxy.kind {
        ProxyKind::Http(http) => {
            let audit = analyze_response_rules(&http.headers.response);
            Ok(Json(audit))
        }
        _ => Err(Error::NotHttpProxy { id }.into()),
    }
}

/// Applies a predefined OWASP security profile to an existing HTTP proxy.
#[utoipa::path(
    post,
    path = "/{id}/security-headers/apply-profile",
    tag = "security_headers",
    operation_id = "apply_proxy_security_header_profile",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    request_body = ApplySecurityProfileRequest,
    responses((status = 200, description = "The updated proxy security audit.", body = SecurityHeadersAudit), NotFoundResponse, ErrorResponses)
)]
pub async fn apply_proxy_security_header_profile(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(body): Json<ApplySecurityProfileRequest>,
) -> Result<Json<SecurityHeadersAudit>, AppError> {
    let mut entry = *state.call(&caller, GetProxy { id }).await?;
    match &mut entry.proxy.kind {
        ProxyKind::Http(http) => {
            let updated_response_rules =
                apply_profile_to_rules(&http.headers.response, body.profile);
            http.headers.response = updated_response_rules;
            let audit = analyze_response_rules(&http.headers.response);
            state.call(&caller, UpdateProxy { entry }).await?;
            Ok(Json(audit))
        }
        _ => Err(Error::NotHttpProxy { id }.into()),
    }
}
