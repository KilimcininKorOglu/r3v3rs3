use async_compression::{
    tokio::bufread::{BrotliEncoder, GzipEncoder, ZstdEncoder},
    Level,
};
use bytes::Bytes;
use futures::TryStreamExt;
use http_body_util::{combinators::BoxBody, BodyExt, StreamBody};
use hyper::{
    body::Frame,
    header::{
        ACCEPT_ENCODING, ACCEPT_RANGES, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH,
        CONTENT_RANGE, CONTENT_TYPE, ETAG, VARY,
    },
    http::HeaderValue,
    HeaderMap, Method, Response, StatusCode,
};
use r3v3rs3_api::compression::{Compression, CompressionAlgorithm};
use std::{io, pin::Pin, sync::Arc};
use tokio::io::AsyncRead;
use tokio_util::io::{ReaderStream, StreamReader};

/// Brotli quality 11, the library default, is too slow for streamed responses.
const BROTLI_QUALITY: i32 = 4;

/// The compression settings of a route and the encoding that the client accepts.
#[derive(Debug)]
pub struct ResponseCompression {
    config: Arc<Compression>,
    algorithm: Option<CompressionAlgorithm>,
    head: bool,
}

impl ResponseCompression {
    pub fn new(config: Arc<Compression>, method: &Method, headers: &HeaderMap) -> Self {
        let algorithm = negotiate(&config.algorithms, headers);
        Self {
            config,
            algorithm,
            head: method == Method::HEAD,
        }
    }

    pub fn apply(
        &self,
        mut res: Response<BoxBody<Bytes, anyhow::Error>>,
    ) -> Response<BoxBody<Bytes, anyhow::Error>> {
        if !is_compressible(&self.config, res.status(), res.headers()) {
            return res;
        }
        append_vary(res.headers_mut());
        let Some(algorithm) = self.algorithm else {
            return res;
        };
        set_encoded_headers(res.headers_mut(), algorithm);
        if self.head {
            return res;
        }
        res.map(|body| encode_body(body, algorithm))
    }
}

/// Returns the algorithm with the highest `q` value in `Accept-Encoding`.
/// Algorithms with the same `q` value keep the configured order.
fn negotiate(
    algorithms: &[CompressionAlgorithm],
    headers: &HeaderMap,
) -> Option<CompressionAlgorithm> {
    let accepted = accepted_codings(headers);
    let quality = |coding: &str| {
        accepted
            .iter()
            .find(|(name, _)| name == coding)
            .or_else(|| accepted.iter().find(|(name, _)| name == "*"))
            .map_or(0, |(_, quality)| *quality)
    };
    let mut best = None;
    let mut best_quality = 0;
    for &algorithm in algorithms {
        let quality = quality(algorithm.as_str());
        if quality > best_quality {
            best = Some(algorithm);
            best_quality = quality;
        }
    }
    best
}

/// Parses `Accept-Encoding` into content codings and `q` values in thousandths.
fn accepted_codings(headers: &HeaderMap) -> Vec<(String, u16)> {
    header_items(headers, ACCEPT_ENCODING)
        .filter_map(parse_coding)
        .collect()
}

fn parse_coding(item: &str) -> Option<(String, u16)> {
    let mut parts = item.split(';');
    let name = parts.next()?.trim().to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    let mut quality = 1000;
    for (key, value) in parts.filter_map(|param| param.split_once('=')) {
        if key.trim().eq_ignore_ascii_case("q") {
            quality = parse_quality(value.trim())?;
        }
    }
    Some((name, quality))
}

/// Parses an RFC 9110 qvalue, e.g. `0.5`, into thousandths.
fn parse_quality(value: &str) -> Option<u16> {
    let quality: f32 = value.parse().ok()?;
    (0.0..=1.0)
        .contains(&quality)
        .then(|| (quality * 1000.0).round() as u16)
}

fn is_compressible(config: &Compression, status: StatusCode, headers: &HeaderMap) -> bool {
    has_full_body(status)
        && !is_encoded(headers)
        && !has_no_transform(headers)
        && has_compressible_type(config, headers)
        && meets_min_size(config, headers)
}

fn has_full_body(status: StatusCode) -> bool {
    !status.is_informational()
        && !matches!(
            status,
            StatusCode::NO_CONTENT | StatusCode::PARTIAL_CONTENT | StatusCode::NOT_MODIFIED
        )
}

fn is_encoded(headers: &HeaderMap) -> bool {
    headers.contains_key(CONTENT_RANGE)
        || headers
            .get_all(CONTENT_ENCODING)
            .iter()
            .any(|value| !value.as_bytes().eq_ignore_ascii_case(b"identity"))
}

fn has_no_transform(headers: &HeaderMap) -> bool {
    header_items(headers, CACHE_CONTROL)
        .any(|directive| directive.eq_ignore_ascii_case("no-transform"))
}

/// Server-sent events are never compressed, because the encoder holds events back.
fn has_compressible_type(config: &Compression, headers: &HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    !content_type
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("text/event-stream")
        && config.compresses_type(content_type)
}

/// A response without `Content-Length` streams a body of unknown size, so it is compressed.
fn meets_min_size(config: &Compression, headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_none_or(|length| length >= config.min_size)
}

/// Returns the trimmed items of a comma-separated header.
fn header_items(
    headers: &HeaderMap,
    name: hyper::header::HeaderName,
) -> impl Iterator<Item = &str> {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
}

fn append_vary(headers: &mut HeaderMap) {
    let listed = header_items(headers, VARY)
        .any(|name| name == "*" || name.eq_ignore_ascii_case("accept-encoding"));
    if !listed {
        headers.append(VARY, HeaderValue::from_static("Accept-Encoding"));
    }
}

fn set_encoded_headers(headers: &mut HeaderMap, algorithm: CompressionAlgorithm) {
    headers.remove(CONTENT_LENGTH);
    headers.remove(ACCEPT_RANGES);
    headers.insert(
        CONTENT_ENCODING,
        HeaderValue::from_static(algorithm.as_str()),
    );
    weaken_etag(headers);
}

/// The encoded body differs from the original body, so a strong validator becomes weak.
fn weaken_etag(headers: &mut HeaderMap) {
    let Some(etag) = headers.get(ETAG) else {
        return;
    };
    if etag.as_bytes().starts_with(b"W/") {
        return;
    }
    let weak = [b"W/", etag.as_bytes()].concat();
    if let Ok(weak) = HeaderValue::from_bytes(&weak) {
        headers.insert(ETAG, weak);
    }
}

fn encode_body(
    body: BoxBody<Bytes, anyhow::Error>,
    algorithm: CompressionAlgorithm,
) -> BoxBody<Bytes, anyhow::Error> {
    let reader = StreamReader::new(body.into_data_stream().map_err(io::Error::other));
    let encoder: Pin<Box<dyn AsyncRead + Send + Sync>> = match algorithm {
        CompressionAlgorithm::Gzip => Box::pin(GzipEncoder::new(reader)),
        CompressionAlgorithm::Brotli => Box::pin(BrotliEncoder::with_quality(
            reader,
            Level::Precise(BROTLI_QUALITY),
        )),
        CompressionAlgorithm::Zstd => Box::pin(ZstdEncoder::new(reader)),
    };
    let frames = ReaderStream::new(encoder)
        .map_ok(Frame::data)
        .map_err(anyhow::Error::from);
    BoxBody::new(StreamBody::new(frames))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::Full;
    use std::io::Read;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| (name.parse().unwrap(), value.parse().unwrap()))
            .collect()
    }

    fn config() -> Arc<Compression> {
        Arc::new(Compression {
            algorithms: vec![CompressionAlgorithm::Zstd, CompressionAlgorithm::Gzip],
            min_size: 10,
            ..Default::default()
        })
    }

    fn accept(value: &str) -> Option<CompressionAlgorithm> {
        negotiate(
            &config().algorithms,
            &headers(&[("accept-encoding", value)]),
        )
    }

    fn response(body: &str, pairs: &[(&str, &str)]) -> Response<BoxBody<Bytes, anyhow::Error>> {
        let mut res = Response::new(BoxBody::new(
            Full::new(Bytes::from(body.to_string())).map_err(Into::into),
        ));
        *res.headers_mut() = headers(pairs);
        res
    }

    #[test]
    fn negotiation_uses_quality_then_configured_order() {
        assert_eq!(accept("gzip, zstd"), Some(CompressionAlgorithm::Zstd));
        assert_eq!(accept("zstd;q=0.5, gzip"), Some(CompressionAlgorithm::Gzip));
        assert_eq!(accept("*"), Some(CompressionAlgorithm::Zstd));
        assert_eq!(accept("*, zstd;q=0"), Some(CompressionAlgorithm::Gzip));
        assert_eq!(accept("gzip;q=0, br"), None);
        assert_eq!(accept("gzip;q=2"), None);
        assert_eq!(accept("identity"), None);
        assert_eq!(negotiate(&config().algorithms, &HeaderMap::new()), None);
    }

    #[test]
    fn ineligible_responses_are_unchanged() {
        let config = config();
        let text = [("content-type", "text/html")];
        assert!(is_compressible(&config, StatusCode::OK, &headers(&text)));
        for pairs in [
            vec![("content-type", "image/png")],
            vec![("content-type", "text/event-stream")],
            vec![("content-type", "text/html"), ("content-length", "9")],
            vec![("content-type", "text/html"), ("content-encoding", "gzip")],
            vec![
                ("content-type", "text/html"),
                ("content-range", "bytes 0-9/99"),
            ],
            vec![
                ("content-type", "text/html"),
                ("cache-control", "public, No-Transform"),
            ],
            vec![],
        ] {
            assert!(!is_compressible(&config, StatusCode::OK, &headers(&pairs)));
        }
        for status in [
            StatusCode::SWITCHING_PROTOCOLS,
            StatusCode::NO_CONTENT,
            StatusCode::PARTIAL_CONTENT,
            StatusCode::NOT_MODIFIED,
        ] {
            assert!(!is_compressible(&config, status, &headers(&text)));
        }
    }

    #[tokio::test]
    async fn gzip_response_is_encoded() {
        let body = "hello compression ".repeat(20);
        let compression = ResponseCompression::new(
            config(),
            &Method::GET,
            &headers(&[("accept-encoding", "gzip")]),
        );
        let res = compression.apply(response(
            &body,
            &[
                ("content-type", "text/plain"),
                ("content-length", "360"),
                ("accept-ranges", "bytes"),
                ("etag", "\"v1\""),
                ("vary", "Origin"),
            ],
        ));
        let headers = res.headers();
        assert_eq!(headers[CONTENT_ENCODING], "gzip");
        assert_eq!(headers[ETAG], "W/\"v1\"");
        assert!(!headers.contains_key(CONTENT_LENGTH));
        assert!(!headers.contains_key(ACCEPT_RANGES));
        assert_eq!(
            headers.get_all(VARY).iter().collect::<Vec<_>>(),
            ["Origin", "Accept-Encoding"]
        );

        let encoded = res.into_body().collect().await.unwrap().to_bytes();
        let mut decoded = String::new();
        flate2::read::GzDecoder::new(&encoded[..])
            .read_to_string(&mut decoded)
            .unwrap();
        assert_eq!(decoded, body);
    }

    #[test]
    fn unsupported_client_gets_vary_only() {
        let compression = ResponseCompression::new(config(), &Method::GET, &HeaderMap::new());
        let res = compression.apply(response(
            "plain text body",
            &[("content-type", "text/plain")],
        ));
        assert!(!res.headers().contains_key(CONTENT_ENCODING));
        assert_eq!(res.headers()[VARY], "Accept-Encoding");
    }
}
