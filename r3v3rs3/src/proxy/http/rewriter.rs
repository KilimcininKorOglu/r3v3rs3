use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::header::{ALT_SVC, CONTENT_TYPE};
use hyper::{body::Body, Response};
use hyper::{
    header::{FORWARDED, VIA},
    http::{header::Entry, HeaderValue},
    HeaderMap,
};
use sailfish::TemplateOnce;
use std::{iter, net::IpAddr, sync::Arc};

use super::client_ip::{ClientAddr, CLIENT_IP_HEADERS};
use super::compression::ResponseCompression;
use super::error::{error_headers, map_error, status_text, ErrorTemplate};
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

        if let Some(host) = &header_host {
            if let Ok(host) = HeaderValue::from_str(host) {
                headers.insert("x-forwarded-host", host);
            }
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
            Ok(mut res) => {
                res.headers_mut().remove(ALT_SVC);
                if let Some(alt_svc) = self.alt_svc() {
                    res.headers_mut()
                        .insert(ALT_SVC, HeaderValue::from_str(&alt_svc)?);
                }
                if let Some((rules, variables)) = &self.header_rules {
                    rules.apply_response(res.headers_mut(), variables);
                }
                // Compression runs after the header rules, so a rule can add `no-transform`.
                let res = res.map(|body| BoxBody::new(body));
                Ok(match &self.compression {
                    Some(compression) => compression.apply(res),
                    None => res,
                })
            }
            Err(err) => error_response(err, self.preferences),
        }
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

fn error_response(
    err: anyhow::Error,
    preferences: PagePreferences,
) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error> {
    let headers = error_headers(&err);
    let code = map_error(err);
    let body = ErrorTemplate {
        code: code.as_u16(),
        text: status_text(code, preferences.locale),
        preferences,
    }
    .render_once()?;
    let mut res = Response::new(BoxBody::new(
        Full::new(Bytes::from(body)).map_err(Into::into),
    ));
    *res.status_mut() = code;
    res.headers_mut().extend(headers);
    res.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
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

    pub fn header_rules(
        mut self,
        rules: Arc<CompiledHeaderRules>,
        variables: HeaderVariables,
    ) -> Self {
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
