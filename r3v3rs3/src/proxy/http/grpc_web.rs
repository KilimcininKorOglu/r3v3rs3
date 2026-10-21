//! Translates browser gRPC-Web requests and trailers for upstream gRPC backends.

use bytes::Bytes;
use http_body_util::Full;
use http_body_util::combinators::BoxBody;
use hyper::body::{Body, Frame, SizeHint};
use hyper::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE, ACCESS_CONTROL_REQUEST_HEADERS,
    CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue, ORIGIN, TE,
};
use hyper::{Method, Request, Response, StatusCode};
use std::pin::Pin;
use std::task::{Context, Poll};

/// Checks if a request uses the gRPC-Web protocol.
pub fn is_grpc_web_request(headers: &HeaderMap) -> bool {
    let has_content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|ct| ct.to_str().ok())
        .is_some_and(|s| s.starts_with("application/grpc-web"));
    has_content_type || headers.contains_key("x-grpc-web")
}

/// Checks if an OPTIONS request is a CORS preflight for gRPC-Web.
pub fn is_grpc_web_preflight<B>(req: &Request<B>) -> bool {
    if req.method() != Method::OPTIONS {
        return false;
    }
    if req.headers().contains_key("x-grpc-web") {
        return true;
    }
    if let Some(req_headers) = req.headers().get(ACCESS_CONTROL_REQUEST_HEADERS)
        && let Ok(headers_str) = req_headers.to_str()
    {
        return headers_str.contains("grpc-web") || headers_str.contains("x-grpc-web");
    }
    req.headers().contains_key(ORIGIN)
}

/// Creates a CORS preflight response for gRPC-Web clients.
pub fn cors_preflight_response(headers: &HeaderMap) -> Response<Full<Bytes>> {
    let origin = headers
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("*");

    let mut res = Response::new(Full::new(Bytes::new()));
    *res.status_mut() = StatusCode::NO_CONTENT;

    let h = res.headers_mut();
    h.insert(
        ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_str(origin).unwrap_or(HeaderValue::from_static("*")),
    );
    h.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST, OPTIONS"),
    );
    h.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type,x-grpc-web,x-user-agent"),
    );
    h.insert(
        ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("grpc-status,grpc-message"),
    );
    h.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("86400"));
    h.insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
    res
}

/// Converts gRPC-Web request headers to native gRPC headers for the upstream server.
pub fn prepare_grpc_request(headers: &mut HeaderMap) {
    if let Some(ct) = headers.get_mut(CONTENT_TYPE)
        && let Ok(ct_str) = ct.to_str()
        && ct_str.starts_with("application/grpc-web")
    {
        *ct = HeaderValue::from_static("application/grpc");
    }
    if !headers.contains_key(TE) {
        headers.insert(TE, HeaderValue::from_static("trailers"));
    }
}

/// Applies gRPC-Web response headers to the client response.
pub fn apply_grpc_web_response_headers(headers: &mut HeaderMap, origin: Option<&str>) {
    headers.remove(CONTENT_LENGTH);
    if let Some(ct) = headers.get_mut(CONTENT_TYPE)
        && let Ok(ct_str) = ct.to_str()
        && ct_str.starts_with("application/grpc")
    {
        *ct = HeaderValue::from_static("application/grpc-web+proto");
    }
    headers.insert(
        ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_str(origin.unwrap_or("*")).unwrap_or(HeaderValue::from_static("*")),
    );
    headers.insert(
        ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("grpc-status,grpc-message"),
    );
}

/// Encodes HTTP trailers into a gRPC-Web trailer frame (flag 0x80 + 4-byte BE length + payload).
pub fn encode_trailers_frame(trailers: &HeaderMap) -> Bytes {
    let mut payload = Vec::new();
    for (name, value) in trailers.iter() {
        let name_str = name.as_str();
        if name_str.starts_with("grpc-") || name_str == "content-type" {
            payload.extend_from_slice(name_str.as_bytes());
            payload.extend_from_slice(b": ");
            payload.extend_from_slice(value.as_bytes());
            payload.extend_from_slice(b"\r\n");
        }
    }
    let len = payload.len() as u32;
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(0x80); // gRPC-Web trailer frame flag
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&payload);
    Bytes::from(frame)
}

/// Response body adapter that frames upstream gRPC trailers into the HTTP response stream.
pub struct GrpcWebBody<B> {
    inner: B,
    trailers_frame: Option<Frame<Bytes>>,
    done: bool,
}

impl<B> Body for GrpcWebBody<B>
where
    B: Body<Data = Bytes, Error = anyhow::Error> + Unpin,
{
    type Data = Bytes;
    type Error = anyhow::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.done {
            return Poll::Ready(None);
        }
        match Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => self.handle_frame(frame),
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err))),
            Poll::Ready(None) => {
                if let Some(frame) = self.trailers_frame.take() {
                    Poll::Ready(Some(Ok(frame)))
                } else {
                    self.done = true;
                    Poll::Ready(None)
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done && self.trailers_frame.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

impl<B> GrpcWebBody<B> {
    fn handle_frame(
        &mut self,
        frame: Frame<Bytes>,
    ) -> Poll<Option<Result<Frame<Bytes>, anyhow::Error>>> {
        match frame.into_data() {
            Ok(data) => Poll::Ready(Some(Ok(Frame::data(data)))),
            Err(frame) => match frame.into_trailers() {
                Ok(trailers) => {
                    self.done = true;
                    let encoded = encode_trailers_frame(&trailers);
                    Poll::Ready(Some(Ok(Frame::data(encoded))))
                }
                Err(_) => Poll::Ready(None),
            },
        }
    }
}

/// Transforms an upstream gRPC response into a browser-compatible gRPC-Web response.
pub fn transform_response<B>(
    res: Response<B>,
    origin: Option<&str>,
) -> Response<BoxBody<Bytes, anyhow::Error>>
where
    B: Body<Data = Bytes, Error = anyhow::Error> + Send + Sync + 'static,
{
    let (mut parts, body) = res.into_parts();
    let trailers_only = parts.headers.contains_key("grpc-status");
    let trailers_frame = if trailers_only {
        Some(Frame::data(encode_trailers_frame(&parts.headers)))
    } else {
        None
    };
    apply_grpc_web_response_headers(&mut parts.headers, origin);
    let web_body = GrpcWebBody {
        inner: BoxBody::new(body),
        trailers_frame,
        done: false,
    };
    Response::from_parts(parts, BoxBody::new(web_body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderName;

    #[test]
    fn test_is_grpc_web_request() {
        let mut h = HeaderMap::new();
        assert!(!is_grpc_web_request(&h));

        h.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/grpc-web+proto"),
        );
        assert!(is_grpc_web_request(&h));

        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        h.insert("x-grpc-web", HeaderValue::from_static("1"));
        assert!(is_grpc_web_request(&h));
    }

    #[test]
    fn test_encode_trailers_frame() {
        let mut trailers = HeaderMap::new();
        trailers.insert(
            HeaderName::from_static("grpc-status"),
            HeaderValue::from_static("0"),
        );
        trailers.insert(
            HeaderName::from_static("grpc-message"),
            HeaderValue::from_static("OK"),
        );
        let frame = encode_trailers_frame(&trailers);
        assert_eq!(frame[0], 0x80); // trailer flag
        let len = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
        assert_eq!(frame.len(), 5 + len);
        let text = std::str::from_utf8(&frame[5..]).unwrap();
        assert!(text.contains("grpc-status: 0\r\n"));
        assert!(text.contains("grpc-message: OK\r\n"));
    }

    #[test]
    fn test_cors_preflight_response() {
        let mut h = HeaderMap::new();
        h.insert(ORIGIN, HeaderValue::from_static("http://localhost:3000"));
        let res = cors_preflight_response(&h);
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            res.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
            "http://localhost:3000"
        );
        assert!(res.headers().contains_key(ACCESS_CONTROL_ALLOW_METHODS));
    }
}
