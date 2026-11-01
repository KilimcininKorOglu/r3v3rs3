use super::{AuthContext, AuthRejection};
use crate::proxy::http::{error::ProxyError, hyper_tls::client::HttpsConnector};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full, Limited};
use hyper::{
    Request, Response, StatusCode, Uri,
    body::Incoming,
    header::{
        CONNECTION, CONTENT_LENGTH, HOST, HeaderMap, HeaderName, HeaderValue, TE, TRAILER,
        TRANSFER_ENCODING, UPGRADE,
    },
};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use r3v3rs3_api::policy::ForwardAuth;
use std::{fmt, net::IpAddr, str::FromStr, sync::Arc, time::Duration};
use tokio_rustls::rustls::ClientConfig;
use tracing::error;

/// Maximum body size of a rejecting auth response. The body is sent to the client.
const MAX_RESPONSE_BODY_SIZE: usize = 64 * 1024;

/// Headers that describe one connection or a request body. The auth request has neither.
const CONNECTION_HEADERS: [HeaderName; 8] = [
    CONNECTION,
    HeaderName::from_static("keep-alive"),
    HeaderName::from_static("proxy-connection"),
    TE,
    TRAILER,
    TRANSFER_ENCODING,
    UPGRADE,
    CONTENT_LENGTH,
];

type AuthClient = Client<HttpsConnector<HttpConnector>, Empty<Bytes>>;

/// Sends a GET request to an external service for each client request. A 2xx response lets the
/// request through. Any other response is sent to the client, so a login redirect works.
pub struct ForwardAuthenticator {
    /// `None` when the configured URL is not a valid URI. Every request is then rejected.
    url: Option<Uri>,
    response_headers: Vec<HeaderName>,
    timeout: Duration,
    client: AuthClient,
}

impl fmt::Debug for ForwardAuthenticator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ForwardAuthenticator")
            .field("url", &self.url)
            .field("response_headers", &self.response_headers)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

enum Verdict {
    Allow(HeaderMap),
    Deny(Response<Full<Bytes>>),
}

fn parse_host_ip(host: &str) -> Option<IpAddr> {
    let clean = host.trim_matches(|c| c == '[' || c == ']');
    clean.parse().ok()
}

pub(crate) fn is_cloud_metadata_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 169 && b == 254
        }
        IpAddr::V6(v6) => {
            let octets = v6.octets();
            octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80
        }
    }
}

pub(crate) fn is_cloud_metadata_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    let trimmed = lower.trim_end_matches('.');
    if trimmed == "metadata.google.internal" || trimmed == "instance-data" {
        return true;
    }
    if let Some(ip) = parse_host_ip(host) {
        return is_cloud_metadata_ip(ip);
    }
    false
}

pub(crate) fn is_loopback_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    let trimmed = lower.trim_end_matches('.');
    if trimmed == "localhost" || trimmed.ends_with(".localhost") || trimmed.ends_with(".local") {
        return true;
    }
    if let Some(ip) = parse_host_ip(host) {
        return ip.is_loopback() || ip.is_unspecified();
    }
    false
}

impl ForwardAuthenticator {
    pub fn new(config: ForwardAuth, tls_client_config: Arc<ClientConfig>) -> Self {
        let url = match Uri::from_str(config.url.0.as_str()) {
            Ok(uri) => {
                let prohibited = uri
                    .host()
                    .map(is_cloud_metadata_host)
                    .unwrap_or(true);
                if prohibited {
                    error!(url = %config.url, "forbidden forward auth url (cloud metadata)");
                    None
                } else {
                    Some(uri)
                }
            }
            Err(err) => {
                error!(url = %config.url, %err, "invalid forward auth url, every request is rejected");
                None
            }
        };
        let response_headers = config
            .response_headers
            .iter()
            .filter_map(|name| {
                HeaderName::from_str(name)
                    .map_err(|err| {
                        error!(%name, %err, "ignoring invalid forward auth response header");
                    })
                    .ok()
            })
            .collect();
        Self {
            url,
            response_headers,
            timeout: config.timeout,
            client: Client::builder(TokioExecutor::new())
                .build(HttpsConnector::new(tls_client_config)),
        }
    }

    pub async fn authorize<B>(
        &self,
        req: &mut Request<B>,
        ctx: &AuthContext<'_>,
    ) -> Result<(), AuthRejection> {
        let auth_req = self.auth_request(req, ctx)?;
        let verdict = tokio::time::timeout(self.timeout, self.ask(auth_req))
            .await
            .map_err(|_| {
                error!(timeout = ?self.timeout, "forward auth request timed out");
                ProxyError::AuthServiceUnavailable
            })??;
        match verdict {
            Verdict::Allow(headers) => {
                self.copy_headers(&headers, req.headers_mut());
                Ok(())
            }
            Verdict::Deny(res) => Err(AuthRejection::Response(Box::new(res))),
        }
    }

    /// Builds the auth request from the client request headers and the `X-Forwarded-*` headers
    /// that describe the client request.
    fn auth_request<B>(
        &self,
        req: &Request<B>,
        ctx: &AuthContext<'_>,
    ) -> Result<Request<Empty<Bytes>>, ProxyError> {
        let url = self.url.clone().ok_or(ProxyError::AuthServiceUnavailable)?;
        let mut headers = req.headers().clone();
        for name in CONNECTION_HEADERS.iter().chain([&HOST]) {
            headers.remove(name);
        }
        let client = ctx.client.to_string();
        let forwarded = [
            ("x-forwarded-method", req.method().as_str()),
            (
                "x-forwarded-proto",
                if ctx.proto == "http" { "http" } else { "https" },
            ),
            ("x-forwarded-host", ctx.host.unwrap_or_default()),
            (
                "x-forwarded-uri",
                req.uri().path_and_query().map_or("/", |uri| uri.as_str()),
            ),
            ("x-forwarded-for", &client),
        ];
        for (name, value) in forwarded {
            let value =
                HeaderValue::from_str(value).map_err(|_| ProxyError::AuthServiceUnavailable)?;
            headers.insert(HeaderName::from_static(name), value);
        }
        let mut auth_req = Request::new(Empty::new());
        *auth_req.uri_mut() = url;
        *auth_req.headers_mut() = headers;
        Ok(auth_req)
    }

    async fn ask(&self, req: Request<Empty<Bytes>>) -> Result<Verdict, ProxyError> {
        let res = self.client.request(req).await.map_err(|err| {
            error!(%err, "forward auth request failed");
            ProxyError::AuthServiceUnavailable
        })?;
        if res.status().is_success() {
            return Ok(Verdict::Allow(res.headers().clone()));
        }
        denied_response(res).await.map(Verdict::Deny)
    }

    /// Replaces the configured headers of the upstream request with the auth response values,
    /// so a client cannot send these headers itself.
    fn copy_headers(&self, from: &HeaderMap, to: &mut HeaderMap) {
        for name in &self.response_headers {
            to.remove(name);
            for value in from.get_all(name) {
                to.append(name.clone(), value.clone());
            }
        }
    }
}

fn is_safe_denied_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "location"
            | "set-cookie"
            | "www-authenticate"
            | "content-type"
            | "content-length"
            | "cache-control"
    )
}

async fn denied_response(res: Response<Incoming>) -> Result<Response<Full<Bytes>>, ProxyError> {
    let status = res.status();
    if !status.is_redirection()
        && status != StatusCode::UNAUTHORIZED
        && status != StatusCode::FORBIDDEN
    {
        error!(%status, "forward auth service rejected with unexpected status, rejecting request");
        return Err(ProxyError::AuthServiceUnavailable);
    }

    let (parts, body) = res.into_parts();
    let body = Limited::new(body, MAX_RESPONSE_BODY_SIZE)
        .collect()
        .await
        .map_err(|err| {
            error!(%err, "failed to read the forward auth response");
            ProxyError::AuthServiceUnavailable
        })?
        .to_bytes();

    let mut sanitized_headers = HeaderMap::new();
    for (name, value) in parts.headers.iter() {
        if is_safe_denied_header(name) {
            sanitized_headers.append(name.clone(), value.clone());
        }
    }

    let mut new_res = Response::new(Full::new(body));
    *new_res.status_mut() = status;
    *new_res.headers_mut() = sanitized_headers;
    Ok(new_res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_rustls::rustls::RootCertStore;

    fn authenticator(url: &str) -> ForwardAuthenticator {
        let tls = ClientConfig::builder()
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth();
        ForwardAuthenticator::new(
            ForwardAuth {
                url: url.parse().unwrap(),
                response_headers: vec!["X-Auth-User".into(), "X-Auth-Groups".into()],
                timeout: Duration::from_secs(5),
            },
            Arc::new(tls),
        )
    }

    #[tokio::test]
    async fn auth_request_describes_the_client_request() {
        let auth = authenticator("http://127.0.0.1:4180/auth?rd=1");
        let req = Request::post("http://example.com/private?page=2")
            .header("cookie", "session=good")
            .header("x-forwarded-for", "203.0.113.66")
            .header(CONTENT_LENGTH, "5")
            .header(HOST, "example.com")
            .body(())
            .unwrap();
        let ctx = AuthContext {
            client: "198.51.100.7".parse().unwrap(),
            host: Some("example.com"),
            proto: "h3",
            base_path: "",
            path_segments: &[],
            preferences: Default::default(),
            vhosts: &[],
        };
        let auth_req = auth.auth_request(&req, &ctx).unwrap();
        let headers = auth_req.headers();
        assert_eq!(auth_req.method(), hyper::Method::GET);
        assert_eq!(auth_req.uri(), "http://127.0.0.1:4180/auth?rd=1");
        assert_eq!(headers["cookie"], "session=good");
        assert_eq!(headers["x-forwarded-method"], "POST");
        assert_eq!(headers["x-forwarded-proto"], "https");
        assert_eq!(headers["x-forwarded-host"], "example.com");
        assert_eq!(headers["x-forwarded-uri"], "/private?page=2");
        assert_eq!(headers["x-forwarded-for"], "198.51.100.7");
        assert!(headers.get(CONTENT_LENGTH).is_none());
        assert!(headers.get(HOST).is_none());
    }

    #[tokio::test]
    async fn copy_headers_replaces_client_values() {
        let auth = authenticator("http://127.0.0.1:4180/auth");
        let mut from = HeaderMap::new();
        from.insert("x-auth-user", "alice".parse().unwrap());
        from.append("x-auth-groups", "admin".parse().unwrap());
        from.append("x-auth-groups", "dev".parse().unwrap());
        from.insert("x-internal", "secret".parse().unwrap());
        let mut to = HeaderMap::new();
        to.insert("x-auth-user", "mallory".parse().unwrap());
        to.insert("x-auth-groups", "root".parse().unwrap());

        auth.copy_headers(&from, &mut to);
        assert_eq!(
            to.get_all("x-auth-user").iter().collect::<Vec<_>>(),
            ["alice"]
        );
        assert_eq!(
            to.get_all("x-auth-groups").iter().collect::<Vec<_>>(),
            ["admin", "dev"]
        );
        assert!(to.get("x-internal").is_none());
    }

    #[test]
    fn metadata_and_loopback_detection() {
        assert!(is_cloud_metadata_host("169.254.169.254"));
        assert!(is_cloud_metadata_host("169.254.1.1"));
        assert!(is_cloud_metadata_host("metadata.google.internal"));
        assert!(is_cloud_metadata_host("instance-data"));
        assert!(!is_cloud_metadata_host("auth.example.com"));

        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("app.localhost"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("[::1]"));
        assert!(!is_loopback_host("auth.example.com"));
    }

    #[test]
    fn safe_denied_headers_filter() {
        assert!(is_safe_denied_header(&hyper::header::LOCATION));
        assert!(is_safe_denied_header(&hyper::header::SET_COOKIE));
        assert!(is_safe_denied_header(&hyper::header::WWW_AUTHENTICATE));
        assert!(!is_safe_denied_header(&hyper::header::SERVER));
        assert!(!is_safe_denied_header(&HeaderName::from_static("x-powered-by")));
    }
}
