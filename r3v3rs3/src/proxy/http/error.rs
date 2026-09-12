use hyper::StatusCode;
use sailfish::TemplateOnce;
use std::time::Duration;
use thiserror::Error;
use tokio_rustls::rustls;

#[derive(Debug, Clone, Error)]
pub enum ProxyError {
    #[error("domain fronting detected")]
    DomainFrontingDetected,

    #[error("no route found")]
    NoRouteFound,

    #[error("client IP address is not allowed")]
    IpNotAllowed,

    #[error("too many requests")]
    TooManyRequests { retry_after: Duration },
}

impl ProxyError {
    fn code(&self) -> StatusCode {
        match self {
            Self::DomainFrontingDetected => StatusCode::MISDIRECTED_REQUEST,
            Self::NoRouteFound => StatusCode::BAD_GATEWAY,
            Self::IpNotAllowed => StatusCode::FORBIDDEN,
            Self::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS,
        }
    }
}

/// Returns the time a rate limited client must wait before the next request.
pub fn retry_after(err: &anyhow::Error) -> Option<Duration> {
    match err.downcast_ref::<ProxyError>() {
        Some(ProxyError::TooManyRequests { retry_after }) => Some(*retry_after),
        _ => None,
    }
}

pub fn map_error(err: anyhow::Error) -> StatusCode {
    if let Some(err) = err.downcast_ref::<ProxyError>() {
        return err.code();
    }
    if let Some(err) = err.downcast_ref::<rustls::Error>() {
        if matches!(err, rustls::Error::InvalidCertificate(_)) {
            return status_code(526);
        } else {
            return status_code(525);
        }
    }
    if let Ok(err) = err.downcast::<hyper::Error>() {
        if err.is_timeout() {
            return StatusCode::GATEWAY_TIMEOUT;
        } else {
            return status_code(523);
        }
    }
    StatusCode::BAD_GATEWAY
}

/// Builds a non-standard status code. Every value passed here is in the valid 100..=999 range.
fn status_code(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY)
}

#[derive(TemplateOnce)]
#[template(path = "error.stpl")]
pub struct ErrorTemplate {
    #[allow(unused_variables)]
    pub code: u16,
}
