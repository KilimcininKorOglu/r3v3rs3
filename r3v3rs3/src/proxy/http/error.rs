use super::page::PagePreferences;
use hyper::header::{HeaderMap, HeaderValue, RETRY_AFTER, WWW_AUTHENTICATE};
use hyper::StatusCode;
use r3v3rs3_api::i18n::Locale;
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

    #[error("the upstream client certificate of the proxy is invalid")]
    UpstreamClientCertInvalid,

    #[error("the upstream server did not respond in time")]
    UpstreamTimeout,

    #[error("every upstream server of the route has an open circuit")]
    NoUpstreamAvailable,
}

impl ProxyError {
    fn code(&self) -> StatusCode {
        match self {
            Self::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
            Self::NoUpstreamAvailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::DomainFrontingDetected => StatusCode::MISDIRECTED_REQUEST,
            Self::NoRouteFound => StatusCode::BAD_GATEWAY,
            Self::IpNotAllowed => StatusCode::FORBIDDEN,
            Self::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::Unauthorized { .. } => StatusCode::UNAUTHORIZED,
            Self::AuthServiceUnavailable | Self::UpstreamClientCertInvalid => {
                StatusCode::BAD_GATEWAY
            }
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

/// The `error_page.<code>` key of every status code that [`map_error`] returns.
const ERROR_PAGE_KEYS: [(u16, &str); 10] = [
    (401, "error_page.401"),
    (403, "error_page.403"),
    (421, "error_page.421"),
    (429, "error_page.429"),
    (502, "error_page.502"),
    (503, "error_page.503"),
    (504, "error_page.504"),
    (523, "error_page.523"),
    (525, "error_page.525"),
    (526, "error_page.526"),
];

/// Returns the reason phrase that the error page shows for the status code. A status code
/// without a key shows its English reason phrase.
pub fn status_text(code: StatusCode, locale: Locale) -> &'static str {
    ERROR_PAGE_KEYS
        .iter()
        .find(|(value, _)| *value == code.as_u16())
        .map_or_else(
            || code.canonical_reason().unwrap_or("Bad Gateway"),
            |(_, key)| locale.t(key),
        )
}

#[derive(TemplateOnce)]
#[template(path = "error.stpl")]
pub struct ErrorTemplate {
    pub code: u16,
    pub text: &'static str,
    pub preferences: PagePreferences,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_names_every_error_status() {
        let text = |code| status_text(code, Locale::En);
        assert_eq!(text(StatusCode::UNAUTHORIZED), "Unauthorized");
        assert_eq!(text(StatusCode::FORBIDDEN), "Forbidden");
        assert_eq!(text(StatusCode::TOO_MANY_REQUESTS), "Too Many Requests");
        assert_eq!(text(StatusCode::GATEWAY_TIMEOUT), "Gateway Timeout");
        assert_eq!(text(StatusCode::MISDIRECTED_REQUEST), "Misdirected Request");
        assert_eq!(text(StatusCode::BAD_GATEWAY), "Bad Gateway");
        assert_eq!(text(StatusCode::SERVICE_UNAVAILABLE), "Service Unavailable");
        assert_eq!(text(status_code(523)), "Origin Is Unreachable");
        assert_eq!(text(status_code(525)), "SSL Handshake Failed");
        assert_eq!(text(status_code(526)), "Invalid SSL Certificate");
        assert_eq!(text(StatusCode::NOT_FOUND), "Not Found");
    }

    #[test]
    fn status_text_uses_the_selected_language() {
        for (code, key) in ERROR_PAGE_KEYS {
            let code = status_code(code);
            assert_eq!(status_text(code, Locale::Tr), Locale::Tr.t(key));
            assert_ne!(status_text(code, Locale::Tr), key);
        }
        assert_ne!(
            status_text(StatusCode::FORBIDDEN, Locale::Tr),
            status_text(StatusCode::FORBIDDEN, Locale::En)
        );
    }
}
