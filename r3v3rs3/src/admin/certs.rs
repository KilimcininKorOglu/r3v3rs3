use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::{
    accounts::{Caller, Permission},
    certs::Cert,
    server::rpc::certs::{
        AddCert, DeleteCert, DeleteCerts, DownloadCert, GetCert, GetCertCoverage, GetCertList,
    },
};
use axum::{
    Extension, Json,
    extract::{Multipart, Path, Query, State},
    http::header::{CONTENT_DISPOSITION, CONTENT_TYPE},
    response::IntoResponse,
};
use r3v3rs3_api::{
    audit::AuditAction,
    cert::{
        CertInfo, CertPostBody, DeleteCertResult, DeleteCertsRequest, SelfSignedCertKind,
        SelfSignedCertRequest, UploadQuery,
    },
    cert_coverage::{CnameVerifyResult, DomainCoverageInfo},
    ct::{
        CheckCtQuery, CtEntriesQuery, CtLogEntry, CtMonitorConfig, CtMonitorStatus,
        DismissCtAlertRequest, UpdateCtConfigRequest,
    },
    error::Error,
    id::ShortId,
};
use std::{ops::Deref, sync::Arc};

/// Lists the certificates.
#[utoipa::path(
    get,
    path = "/",
    tag = "certs",
    operation_id = "list_certs",
    responses((status = 200, description = "The certificates.", body = Vec<CertInfo>), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<Vec<CertInfo>>>, AppError> {
    Ok(Json(state.call(&caller, GetCertList).await?))
}

/// Parameters for querying public DNS CNAME verification.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct CnameVerifyQuery {
    pub domain: String,
    pub expected_target: Option<String>,
}

/// Checks certificate coverage and wildcard auto-sharing status for a list of domains.
#[utoipa::path(
    post,
    path = "/coverage",
    tag = "certs",
    operation_id = "check_cert_coverage",
    request_body = Vec<String>,
    responses((status = 200, description = "The certificate coverage report for each domain.", body = Vec<DomainCoverageInfo>), ErrorResponses)
)]
pub async fn check_coverage(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(domains): Json<Vec<String>>,
) -> Result<Json<Box<Vec<DomainCoverageInfo>>>, AppError> {
    Ok(Json(
        state.call(&caller, GetCertCoverage { domains }).await?,
    ))
}

/// Queries public DNS to verify if a domain's CNAME record points to the expected target.
#[utoipa::path(
    get,
    path = "/verify_cname",
    tag = "certs",
    operation_id = "verify_cname",
    params(CnameVerifyQuery),
    responses((status = 200, description = "The CNAME verification result.", body = CnameVerifyResult), ErrorResponses)
)]
pub async fn verify_cname(
    State(state): State<AppState>,
    Extension(_caller): Extension<Caller>,
    Query(query): Query<CnameVerifyQuery>,
) -> Result<Json<CnameVerifyResult>, AppError> {
    let fallback = {
        let data = state.data.lock().await;
        data.config
            .public_url
            .as_ref()
            .and_then(|u| url::Url::parse(u.as_str()).ok())
            .and_then(|u| u.host_str().map(ToString::to_string))
            .unwrap_or_else(|| "localhost".to_string())
    };
    let result = crate::certs::cname::verify_domain_cname(
        &query.domain,
        query.expected_target.as_deref(),
        &fallback,
    )
    .await;
    Ok(Json(result))
}

/// Returns one certificate.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "certs",
    operation_id = "get_cert",
    params(("id" = ShortId, Path, description = "Certificate id.")),
    responses((status = 200, description = "The certificate.", body = CertInfo), NotFoundResponse, ErrorResponses)
)]
pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<CertInfo>>, AppError> {
    let cert = state.call(&caller, GetCert { id }).await?;
    Ok(Json(Box::new(cert.info())))
}

/// Creates a self-signed server or client certificate. Without `ca_cert`, a new CA certificate signs it.
#[utoipa::path(
    post,
    path = "/self_sign",
    tag = "certs",
    operation_id = "self_sign_cert",
    request_body = SelfSignedCertRequest,
    responses((status = 200, description = "The certificate is created."), NotFoundResponse, ErrorResponses)
)]
pub async fn self_sign(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<SelfSignedCertRequest>,
) -> Result<Json<Box<()>>, AppError> {
    let cert = if let Some(ca_cert) = request.ca_cert {
        let ca = state.call(&caller, GetCert { id: ca_cert }).await?;
        sign(&request, &ca)?
    } else {
        let ca = Arc::new(Cert::new_ca()?);
        state.call(&caller, AddCert { cert: ca.clone() }).await?;
        sign(&request, &ca)?
    };
    let cert = Arc::new(cert);
    Ok(Json(state.call(&caller, AddCert { cert }).await?))
}

fn sign(request: &SelfSignedCertRequest, ca: &Cert) -> Result<Cert, Error> {
    match request.kind {
        SelfSignedCertKind::Server => Cert::new_self_signed(&request.san, ca),
        SelfSignedCertKind::Client => Cert::new_client(&request.san, ca),
    }
}

/// Uploads a PEM certificate chain and an optional PEM private key.
#[utoipa::path(
    post,
    path = "/upload",
    tag = "certs",
    operation_id = "upload_cert",
    params(UploadQuery),
    request_body(content = CertPostBody, content_type = "multipart/form-data"),
    responses((status = 200, description = "The certificate is added."), ErrorResponses)
)]
pub async fn upload(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Query(query): Query<UploadQuery>,
    mut multipart: Multipart,
) -> Result<Json<Box<()>>, AppError> {
    let mut chain = Vec::new();
    let mut key = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("chain") {
            if let Ok(buf) = field.bytes().await {
                chain = buf.to_vec();
            }
        } else if field.name() == Some("key")
            && let Ok(buf) = field.bytes().await
        {
            key = buf.to_vec();
        }
    }

    let key = if key.is_empty() { None } else { Some(key) };
    let cert = Arc::new(Cert::new(query.kind, chain, key)?);
    Ok(Json(state.call(&caller, AddCert { cert }).await?))
}

/// Deletes a certificate.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "certs",
    operation_id = "delete_cert",
    params(("id" = ShortId, Path, description = "Certificate id.")),
    responses((status = 200, description = "The certificate is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(&caller, DeleteCert { id }).await?))
}

/// Deletes each listed certificate that nothing uses. A certificate that a port, a proxy or a
/// discovery provider uses stays, and the response tells what happened to each id.
#[utoipa::path(
    post,
    path = "/delete",
    tag = "certs",
    operation_id = "delete_certs",
    request_body = DeleteCertsRequest,
    responses((status = 200, description = "The result of each certificate.", body = Vec<DeleteCertResult>), ErrorResponses)
)]
pub async fn delete_many(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<DeleteCertsRequest>,
) -> Result<Json<Box<Vec<DeleteCertResult>>>, AppError> {
    let ids = request.ids;
    Ok(Json(state.call(&caller, DeleteCerts { ids }).await?))
}

/// Downloads the certificate chain and the private key as a `tar.gz` archive.
#[utoipa::path(
    get,
    path = "/{id}/download",
    tag = "certs",
    operation_id = "download_cert",
    params(("id" = ShortId, Path, description = "Certificate id.")),
    responses((status = 200, description = "The archive.", content_type = "application/gzip", body = Vec<u8>), NotFoundResponse, ErrorResponses)
)]
pub async fn download(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<impl IntoResponse, AppError> {
    let file = state.call(&caller, DownloadCert { id }).await?;
    let headers = [
        (CONTENT_TYPE, "application/gzip".to_string()),
        (
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{id}.tar.gz\""),
        ),
    ];
    Ok((headers, file.deref().clone()))
}

/// Returns the Certificate Transparency monitor configuration.
#[utoipa::path(
    get,
    path = "/ct/config",
    tag = "certs",
    operation_id = "get_ct_config",
    responses((status = 200, description = "The CT monitor configuration.", body = CtMonitorConfig), ErrorResponses)
)]
pub async fn get_ct_config(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<CtMonitorConfig>, AppError> {
    caller.authorize(Permission::Read)?;
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    let monitor = crate::certs::ct::get_or_init_state(&config_dir);
    Ok(Json(monitor.get_config()))
}

/// Updates the Certificate Transparency monitor configuration.
#[utoipa::path(
    put,
    path = "/ct/config",
    tag = "certs",
    operation_id = "update_ct_config",
    request_body = UpdateCtConfigRequest,
    responses((status = 200, description = "The updated configuration.", body = CtMonitorConfig), ErrorResponses)
)]
pub async fn put_ct_config(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<UpdateCtConfigRequest>,
) -> Result<Json<CtMonitorConfig>, AppError> {
    caller.authorize(Permission::Edit)?;
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    let monitor = crate::certs::ct::get_or_init_state(&config_dir);
    monitor.save_config(body.config.clone())?;
    let record = crate::audit::AuditRecord::new(AuditAction::UpdateCtMonitorConfig)
        .summary("Updated Certificate Transparency monitor configuration");
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(monitor.get_config()))
}

/// Returns the runtime status of the Certificate Transparency monitor.
#[utoipa::path(
    get,
    path = "/ct/status",
    tag = "certs",
    operation_id = "get_ct_status",
    responses((status = 200, description = "The CT monitor status.", body = CtMonitorStatus), ErrorResponses)
)]
pub async fn get_ct_status(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<CtMonitorStatus>, AppError> {
    caller.authorize(Permission::Read)?;
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    let monitor = crate::certs::ct::get_or_init_state(&config_dir);
    Ok(Json(monitor.get_status()))
}

/// Lists Certificate Transparency log records discovered for monitored domains.
#[utoipa::path(
    get,
    path = "/ct/entries",
    tag = "certs",
    operation_id = "list_ct_entries",
    params(CtEntriesQuery),
    responses((status = 200, description = "List of CT log entries.", body = Vec<CtLogEntry>), ErrorResponses)
)]
pub async fn list_ct_entries(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Query(query): Query<CtEntriesQuery>,
) -> Result<Json<Vec<CtLogEntry>>, AppError> {
    caller.authorize(Permission::Read)?;
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    let monitor = crate::certs::ct::get_or_init_state(&config_dir);
    Ok(Json(monitor.list_entries(query)))
}

/// Triggers an immediate Certificate Transparency log check.
#[utoipa::path(
    post,
    path = "/ct/check",
    tag = "certs",
    operation_id = "check_ct_now",
    params(CheckCtQuery),
    responses((status = 200, description = "Number of new rogue certificates found.", body = usize), ErrorResponses)
)]
pub async fn check_ct_now(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Query(query): Query<CheckCtQuery>,
) -> Result<Json<usize>, AppError> {
    caller.authorize(Permission::Edit)?;
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    let monitor = crate::certs::ct::get_or_init_state(&config_dir);
    let count = monitor.run_check(&state, query.domain.as_deref()).await?;
    let record = crate::audit::AuditRecord::new(AuditAction::CheckCtLogs).summary(format!(
        "Triggered Certificate Transparency check, {} new rogue certs found",
        count
    ));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(count))
}

/// Dismisses or updates the status of a Certificate Transparency alert.
#[utoipa::path(
    post,
    path = "/ct/dismiss",
    tag = "certs",
    operation_id = "dismiss_ct_alert",
    request_body = DismissCtAlertRequest,
    responses((status = 200, description = "Alert dismissed successfully.", body = bool), ErrorResponses)
)]
pub async fn dismiss_ct_alert(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<DismissCtAlertRequest>,
) -> Result<Json<bool>, AppError> {
    caller.authorize(Permission::Edit)?;
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    let monitor = crate::certs::ct::get_or_init_state(&config_dir);
    monitor.dismiss_alert(&body.id, body.status)?;
    let record = crate::audit::AuditRecord::new(AuditAction::DismissCtAlert).summary(format!(
        "Dismissed Certificate Transparency alert for entry {}",
        body.id
    ));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(true))
}
