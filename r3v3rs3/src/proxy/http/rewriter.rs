use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::header::{ALT_SVC, CACHE_CONTROL, CONTENT_TYPE, SET_COOKIE};
use hyper::{
    HeaderMap,
    header::{FORWARDED, VIA},
    http::{HeaderValue, header::Entry},
};
use hyper::{Response, StatusCode, body::Body};
use r3v3rs3_api::proxy::CustomErrorPage;
use sailfish::TemplateOnce;
use std::{iter, net::IpAddr, sync::Arc, sync::PoisonError};

use super::affinity::CookieSlot;
use super::client_ip::{CLIENT_IP_HEADERS, ClientAddr};
use super::compression::ResponseCompression;
use super::error::{ErrorTemplate, error_headers, map_error, status_text};
use super::header_rules::{CompiledHeaderRules, HeaderVariables};
use super::page::PagePreferences;

#[derive(Default, Debug)]
pub struct RequestRewriter {
    set_via: Option<HeaderValue>,
}

impl RequestRewriter {
    pub fn builder() -> RequestRewriterBuilder {
        Default::default()
    }

    fn remove_untrusted_headers(&self, headers: &mut HeaderMap) {
        for key in CLIENT_IP_HEADERS {
            if let Entry::Occupied(entry) = headers.entry(*key) {
                entry.remove_entry_mult();
            }
        }
    }

    fn parse_x_forwarded_for(&self, headers: &mut HeaderMap) -> Vec<IpAddr> {
        if let Entry::Occupied(entry) = headers.entry("x-forwarded-for") {
            return entry
                .remove_entry_mult()
                .1
                .flat_map(|v| {
                    v.to_str()
                        .ok()
                        .unwrap_or_default()
                        .split(',')
                        .filter_map(|ip| ip.trim().parse().ok())
                        .collect::<Vec<IpAddr>>()
                })
                .collect();
        }
        Vec::new()
    }

    fn parse_forwarded(&self, headers: &mut HeaderMap) -> Vec<String> {
        if let Entry::Occupied(entry) = headers.entry(FORWARDED) {
            return entry
                .remove_entry_mult()
                .1
                .flat_map(|v| {
                    v.to_str()
                        .ok()
                        .unwrap_or_default()
                        .split(',')
                        .map(|item| item.trim().to_string())
                        .collect::<Vec<String>>()
                })
                .collect();
        }
        Vec::new()
    }

    pub fn pre_process(
        &self,
        headers: &mut HeaderMap,
        client: &ClientAddr,
        header_host: Option<String>,
        forwarded_proto: &'static str,
    ) {
        let mut x_forwarded_for = Vec::new();
        let mut forwarded = Vec::new();
        let remote_addr = client.peer;

        if client.trusted_peer {
            x_forwarded_for = self.parse_x_forwarded_for(headers);
            forwarded = self.parse_forwarded(headers);
            if let Ok(real_ip) = HeaderValue::from_str(&client.ip.to_string()) {
                headers.insert("x-real-ip", real_ip);
            }
        } else {
            self.remove_untrusted_headers(headers);
        }

        if forwarded.is_empty() {
            forwarded = x_forwarded_for
                .iter()
                .map(|ip| forwarded_for_directive(*ip))
                .collect();
        }
        if let Ok(forwarded_value) = HeaderValue::from_str(
            &forwarded
                .into_iter()
                .chain(iter::once(forwarded_for_directive(remote_addr)))
                .chain(
                    header_host
                        .as_ref()
                        .map(|host| forwarded_host_directive(host)),
                )
                .chain(iter::once(forwarded_proto_directive(forwarded_proto)))
                .collect::<Vec<_>>()
                .join(", "),
        ) {
            headers.insert(FORWARDED, forwarded_value);
        }

        if let Ok(x_forwarded_value) = HeaderValue::from_str(
            &x_forwarded_for
                .iter()
                .chain(iter::once(&remote_addr))
                .map(|ip| ip.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ) {
            headers.insert("x-forwarded-for", x_forwarded_value);
        }

        headers.insert(
            "x-forwarded-proto",
            HeaderValue::from_static(forwarded_proto),
        );

        if let Some(host) = &header_host
            && let Ok(host) = HeaderValue::from_str(host)
        {
            headers.insert("x-forwarded-host", host);
        }
    }

    pub fn post_process(&self, headers: &mut HeaderMap) {
        if let Some(via) = &self.set_via {
            headers.insert(VIA, via.clone());
        }
    }
}

#[derive(Default)]
pub struct RequestRewriterBuilder {
    inner: RequestRewriter,
}

impl RequestRewriterBuilder {
    pub fn set_via(mut self, via: HeaderValue) -> Self {
        self.inner.set_via = Some(via);
        self
    }

    pub fn build(self) -> RequestRewriter {
        self.inner
    }
}

fn forwarded_for_directive(addr: IpAddr) -> String {
    if addr.is_ipv6() {
        format!("for=\"[{addr}]\"")
    } else {
        format!("for={addr}")
    }
}

fn forwarded_host_directive(host: &str) -> String {
    format!("host={host}")
}

fn forwarded_proto_directive(proto: &str) -> String {
    format!("proto={proto}")
}

#[derive(Default, Debug)]
pub struct ResponseRewriter {
    https_port: Option<u16>,
    quic_port: Option<u16>,
    /// Response header rules of the route and the variable values of the request.
    header_rules: Option<(Arc<CompiledHeaderRules>, HeaderVariables)>,
    compression: Option<ResponseCompression>,
    /// Language and theme of the error page.
    preferences: PagePreferences,
    /// The sticky cookie that the upstream request sets. It is added outside the cache, because
    /// the cache does not store a response with `Set-Cookie`.
    sticky_cookie: Option<CookieSlot>,
    error_pages: Arc<[CustomErrorPage]>,
    grpc_web: Option<Option<String>>,
    variables: Option<HeaderVariables>,
}

impl ResponseRewriter {
    pub fn builder() -> ResponseRewriterBuilder {
        Default::default()
    }

    pub fn map_response<B>(
        &self,
        res: Result<Response<B>, anyhow::Error>,
    ) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error>
    where
        B: Body<Data = Bytes, Error = anyhow::Error> + Send + Sync + 'static,
    {
        match res {
            Ok(res) => {
                if let Some(custom) = self
                    .error_pages
                    .iter()
                    .find(|p| p.status == res.status().as_u16())
                {
                    let mut res = custom_response_body(custom, res, self.variables.as_ref());
                    self.apply_post_headers(res.headers_mut());
                    return Ok(res);
                }
                let mut res = res;
                res.headers_mut().remove(ALT_SVC);
                if let Some(alt_svc) = self.alt_svc() {
                    res.headers_mut()
                        .insert(ALT_SVC, HeaderValue::from_str(&alt_svc)?);
                }
                self.apply_post_headers(res.headers_mut());
                let res = if let Some(origin) = &self.grpc_web {
                    super::grpc_web::transform_response(res, origin.as_deref())
                } else {
                    res.map(BoxBody::new)
                };
                Ok(match &self.compression {
                    Some(compression) => compression.apply(res),
                    None => res,
                })
            }
            Err(err) => error_response(
                err,
                self.preferences,
                &self.error_pages,
                self.variables.as_ref(),
            ),
        }
    }

    fn apply_post_headers(&self, headers: &mut HeaderMap) {
        if let Some((rules, variables)) = &self.header_rules {
            rules.apply_response(headers, variables);
        }
        if let Some(cookie) = self.sticky_cookie_value() {
            headers.append(SET_COOKIE, cookie);
        }
    }

    fn sticky_cookie_value(&self) -> Option<HeaderValue> {
        self.sticky_cookie
            .as_ref()?
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn alt_svc(&self) -> Option<String> {
        match (self.https_port, self.quic_port) {
            (Some(https), Some(quic)) => Some(format!(
                "h2=\":{}\", h3=\":{}\", h3-25=\":{}\"",
                https, quic, quic
            )),
            (Some(https), None) => Some(format!("h2=\":{}\"", https)),
            (None, Some(quic)) => Some(format!("h3=\":{}\", h3-25=\":{}\"", quic, quic)),
            _ => None,
        }
    }
}

fn is_html_body(body: &[u8], body_str: &str) -> bool {
    body.starts_with(b"<!DOCTYPE")
        || body.starts_with(b"<html")
        || body_str.contains("</html>")
        || body_str.contains("</div>")
        || body_str.contains("</p>")
        || body_str.contains("</h1>")
}

fn custom_response_body<B>(
    custom: &CustomErrorPage,
    mut res: Response<B>,
    variables: Option<&HeaderVariables>,
) -> Response<BoxBody<Bytes, anyhow::Error>> {
    let status = custom.status;
    let message = StatusCode::from_u16(status)
        .ok()
        .and_then(|c| c.canonical_reason())
        .unwrap_or("Error");
    let (req_id, host, client_ip) = match variables {
        Some(v) => (
            v.request_id.as_str(),
            v.host.as_str(),
            v.client_ip.to_string(),
        ),
        None => ("-", "-", "-".to_string()),
    };
    let rendered = r3v3rs3_api::error_page_theme::substitute_error_page_variables(
        &custom.body,
        status,
        message,
        req_id,
        host,
        &client_ip,
    );
    let body = rendered.as_bytes().to_vec();
    let content_type = if is_html_body(&body, &rendered) {
        HeaderValue::from_static("text/html; charset=utf-8")
    } else {
        HeaderValue::from_static("text/plain; charset=utf-8")
    };
    res.headers_mut().insert(CONTENT_TYPE, content_type);
    res.headers_mut()
        .insert(hyper::header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    res.headers_mut().remove(hyper::header::CONTENT_ENCODING);
    res.map(|_| BoxBody::new(Full::new(Bytes::from(body)).map_err(Into::into)))
}

fn render_error_body(
    code: StatusCode,
    preferences: PagePreferences,
    error_pages: &[CustomErrorPage],
    variables: Option<&HeaderVariables>,
) -> Result<(Vec<u8>, HeaderValue), anyhow::Error> {
    if let Some(custom) = error_pages.iter().find(|p| p.status == code.as_u16()) {
        let status = custom.status;
        let message = code.canonical_reason().unwrap_or("Error");
        let (req_id, host, client_ip) = match variables {
            Some(v) => (
                v.request_id.as_str(),
                v.host.as_str(),
                v.client_ip.to_string(),
            ),
            None => ("-", "-", "-".to_string()),
        };
        let rendered = r3v3rs3_api::error_page_theme::substitute_error_page_variables(
            &custom.body,
            status,
            message,
            req_id,
            host,
            &client_ip,
        );
        let body = rendered.as_bytes().to_vec();
        let content_type = if is_html_body(&body, &rendered) {
            HeaderValue::from_static("text/html; charset=utf-8")
        } else {
            HeaderValue::from_static("text/plain; charset=utf-8")
        };
        Ok((body, content_type))
    } else {
        let body = ErrorTemplate {
            code: code.as_u16(),
            text: status_text(code, preferences.locale),
            preferences,
        }
        .render_once()?;
        Ok((
            body.into_bytes(),
            HeaderValue::from_static("text/html; charset=utf-8"),
        ))
    }
}

fn error_response(
    err: anyhow::Error,
    preferences: PagePreferences,
    error_pages: &[CustomErrorPage],
    variables: Option<&HeaderVariables>,
) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error> {
    let headers = error_headers(&err);
    let code = map_error(err);
    let (body, content_type) = render_error_body(code, preferences, error_pages, variables)?;
    let mut res = Response::new(BoxBody::new(
        Full::new(Bytes::from(body)).map_err(Into::into),
    ));
    *res.status_mut() = code;
    res.headers_mut().extend(headers);
    res.headers_mut().insert(CONTENT_TYPE, content_type);
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

#[derive(Default)]
pub struct ResponseRewriterBuilder {
    inner: ResponseRewriter,
}

impl ResponseRewriterBuilder {
    pub fn https_port(mut self, port: Option<u16>) -> Self {
        self.inner.https_port = port;
        self
    }

    pub fn quic_port(mut self, port: Option<u16>) -> Self {
        self.inner.quic_port = port;
        self
    }

    pub fn variables(mut self, variables: HeaderVariables) -> Self {
        self.inner.variables = Some(variables);
        self
    }

    pub fn header_rules(
        mut self,
        rules: Arc<CompiledHeaderRules>,
        variables: HeaderVariables,
    ) -> Self {
        self.inner.variables = Some(variables.clone());
        self.inner.header_rules = Some((rules, variables));
        self
    }

    pub fn compression(mut self, compression: Option<ResponseCompression>) -> Self {
        self.inner.compression = compression;
        self
    }

    pub fn preferences(mut self, preferences: PagePreferences) -> Self {
        self.inner.preferences = preferences;
        self
    }

    pub fn sticky_cookie(mut self, cookie: Option<CookieSlot>) -> Self {
        self.inner.sticky_cookie = cookie;
        self
    }

    pub fn error_pages(mut self, pages: Arc<[CustomErrorPage]>) -> Self {
        self.inner.error_pages = pages;
        self
    }

    pub fn grpc_web(mut self, origin: Option<String>) -> Self {
        self.inner.grpc_web = Some(origin);
        self
    }

    pub fn build(self) -> ResponseRewriter {
        self.inner
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn trusted(peer: IpAddr, ip: &str) -> ClientAddr {
        ClientAddr {
            peer,
            ip: ip.parse().unwrap(),
            trusted_peer: true,
        }
    }

    #[test]
    fn the_error_page_is_never_stored() {
        let err = anyhow::Error::new(super::super::error::ProxyError::NoUpstreamServers);
        let res = error_response(err, PagePreferences::default(), &[], None).unwrap();
        assert_eq!(res.status(), 502);
        assert_eq!(res.headers()[CACHE_CONTROL], "no-store");
        assert_eq!(res.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
    }

    #[test]
    fn custom_error_page_replaces_response_body() {
        let err = anyhow::Error::new(super::super::error::ProxyError::NoUpstreamServers);
        let pages = [r3v3rs3_api::proxy::CustomErrorPage {
            status: 502,
            body: "<h1>Custom 502 Bad Gateway</h1>".to_string(),
        }];
        let res = error_response(err, PagePreferences::default(), &pages, None).unwrap();
        assert_eq!(res.status(), 502);
        assert_eq!(res.headers()[CACHE_CONTROL], "no-store");
        assert_eq!(res.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
    }

    #[tokio::test]
    async fn test_custom_error_page_substitutes_variables() {
        use http_body_util::BodyExt;
        let err = anyhow::Error::new(super::super::error::ProxyError::NoUpstreamServers);
        let pages = [r3v3rs3_api::proxy::CustomErrorPage {
            status: 502,
            body: "<title>{{status}} {{message}}</title><p>Req: {{request_id}} Host: {{host}} IP: {{client_ip}}</p>".to_string(),
        }];
        let vars = HeaderVariables {
            client_ip: "203.0.113.19".parse().unwrap(),
            host: "test.example.com".to_string(),
            scheme: "https",
            request_id: "req-abc-xyz".to_string(),
            route: "/api".to_string(),
            client_cert_subject: String::new(),
            client_cert_fingerprint: String::new(),
        };
        let res = error_response(err, PagePreferences::default(), &pages, Some(&vars)).unwrap();
        assert_eq!(res.status(), 502);
        let body_bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("<title>502 Bad Gateway</title>"));
        assert!(body_str.contains("Req: req-abc-xyz"));
        assert!(body_str.contains("Host: test.example.com"));
        assert!(body_str.contains("IP: 203.0.113.19"));
    }

    #[test]
    fn test_header_rewriter_pre_process() {
        let forwarded_proto = "http";
        let localhost = Ipv4Addr::new(127, 0, 0, 1).into();

        let mut headers = HeaderMap::new();
        headers.append("x-forwarded-for", "192.168.0.1".parse().unwrap());
        headers.append("x-real-ip", "192.168.0.1".parse().unwrap());
        headers.append("cf-connecting-ip", "192.168.0.1".parse().unwrap());

        let rewriter = RequestRewriter::builder().build();
        rewriter.pre_process(
            &mut headers,
            &ClientAddr::direct(localhost),
            Some("example.com".into()),
            forwarded_proto,
        );
        assert_eq!(headers.get("x-forwarded-for").unwrap(), "127.0.0.1");
        assert_eq!(headers.get("x-real-ip"), None);
        assert_eq!(headers.get("cf-connecting-ip"), None);
        assert_eq!(headers.get("x-forwarded-proto").unwrap(), "http");
        assert_eq!(headers.get("x-forwarded-host").unwrap(), "example.com");

        let mut headers = HeaderMap::new();
        headers.append(FORWARDED, "for=192.168.0.1".parse().unwrap());

        let rewriter = RequestRewriter::builder().build();
        rewriter.pre_process(
            &mut headers,
            &trusted(localhost, "192.168.0.1"),
            Some("example.com".into()),
            forwarded_proto,
        );
        assert_eq!(
            headers.get(FORWARDED).unwrap(),
            "for=192.168.0.1, for=127.0.0.1, host=example.com, proto=http"
        );
        assert_eq!(headers.get("x-forwarded-for").unwrap(), "127.0.0.1");
        assert_eq!(headers.get("x-real-ip").unwrap(), "192.168.0.1");
        assert_eq!(headers.get("x-forwarded-proto").unwrap(), "http");
        assert_eq!(headers.get("x-forwarded-host").unwrap(), "example.com");

        let mut headers = HeaderMap::new();
        headers.append("x-forwarded-for", "192.168.0.1".parse().unwrap());

        let rewriter = RequestRewriter::builder().build();
        rewriter.pre_process(
            &mut headers,
            &trusted(localhost, "192.168.0.1"),
            Some("example.com".into()),
            forwarded_proto,
        );
        assert_eq!(
            headers.get("x-forwarded-for").unwrap(),
            "192.168.0.1, 127.0.0.1"
        );
        assert_eq!(headers.get("x-forwarded-proto").unwrap(), "http");
        assert_eq!(headers.get("x-forwarded-host").unwrap(), "example.com");

        let mut headers = HeaderMap::new();
        headers.append("x-forwarded-for", "192.168.0.1".parse().unwrap());

        let rewriter = RequestRewriter::builder().build();
        rewriter.pre_process(
            &mut headers,
            &trusted(Ipv6Addr::LOCALHOST.into(), "192.168.0.1"),
            Some("example.com".into()),
            forwarded_proto,
        );
        assert_eq!(
            headers.get(FORWARDED).unwrap(),
            "for=192.168.0.1, for=\"[::1]\", host=example.com, proto=http"
        );
        assert_eq!(headers.get("x-forwarded-for").unwrap(), "192.168.0.1, ::1");
        assert_eq!(headers.get("x-forwarded-proto").unwrap(), "http");
        assert_eq!(headers.get("x-forwarded-host").unwrap(), "example.com");
    }

    #[test]
    fn test_header_rewriter_post_process() {
        let mut headers = HeaderMap::new();
        let rewriter = RequestRewriter::builder()
            .set_via("r3v3rs3".parse().unwrap())
            .build();
        rewriter.post_process(&mut headers);
        assert_eq!(headers.get("via").unwrap(), "r3v3rs3");
    }
}
