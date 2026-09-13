use hyper::header::{HeaderMap, HeaderValue, RETRY_AFTER, WWW_AUTHENTICATE};
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

    #[error("authentication required")]
    Unauthorized { challenge: HeaderValue },

    #[error("authentication service is unavailable")]
    AuthServiceUnavailable,
}

impl ProxyError {
    fn code(&self) -> StatusCode {
        match self {
            Self::DomainFrontingDetected => StatusCode::MISDIRECTED_REQUEST,
            Self::NoRouteFound => StatusCode::BAD_GATEWAY,
            Self::IpNotAllowed => StatusCode::FORBIDDEN,
            Self::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::Unauthorized { .. } => StatusCode::UNAUTHORIZED,
            Self::AuthServiceUnavailable => StatusCode::BAD_GATEWAY,
        }
    }
}

/// Returns the headers that the error response must carry: `Retry-After` for a rate limited
/// client and `WWW-Authenticate` for an unauthenticated client.
pub fn error_headers(err: &anyhow::Error) -> HeaderMap {
    let mut headers = HeaderMap::new();
    match err.downcast_ref::<ProxyError>() {
        Some(ProxyError::TooManyRequests { retry_after }) => {
            let seconds = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
            headers.insert(RETRY_AFTER, HeaderValue::from(seconds.max(1)));
        }
        Some(ProxyError::Unauthorized { challenge }) => {
            headers.insert(WWW_AUTHENTICATE, challenge.clone());
        }
        _ => {}
    }
    headers
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

/// Returns the reason phrase that the error page shows for the status code.
pub fn status_text(code: StatusCode) -> &'static str {
    match code.as_u16() {
        523 => "Origin Is Unreachable",
        525 => "SSL Handshake Failed",
        526 => "Invalid SSL Certificate",
        _ => code.canonical_reason().unwrap_or("Bad Gateway"),
    }
}

#[derive(TemplateOnce)]
#[template(path = "error.stpl")]
pub struct ErrorTemplate {
    pub code: u16,
    pub text: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_names_every_error_status() {
        assert_eq!(status_text(StatusCode::UNAUTHORIZED), "Unauthorized");
        assert_eq!(status_text(StatusCode::FORBIDDEN), "Forbidden");
        assert_eq!(
            status_text(StatusCode::TOO_MANY_REQUESTS),
            "Too Many Requests"
        );
        assert_eq!(status_text(StatusCode::GATEWAY_TIMEOUT), "Gateway Timeout");
        assert_eq!(
            status_text(StatusCode::MISDIRECTED_REQUEST),
            "Misdirected Request"
        );
        assert_eq!(status_text(StatusCode::BAD_GATEWAY), "Bad Gateway");
        assert_eq!(status_text(status_code(523)), "Origin Is Unreachable");
        assert_eq!(status_text(status_code(525)), "SSL Handshake Failed");
        assert_eq!(status_text(status_code(526)), "Invalid SSL Certificate");
    }
}
