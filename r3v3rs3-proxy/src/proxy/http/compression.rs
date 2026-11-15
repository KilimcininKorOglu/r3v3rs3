use async_compression::{
    Level,
    tokio::bufread::{BrotliEncoder, GzipEncoder, ZstdEncoder},
};
use bytes::Bytes;
use futures::TryStreamExt;
use http_body_util::{BodyExt, StreamBody, combinators::BoxBody};
use hyper::{
    HeaderMap, Method, Response, StatusCode,
    body::Frame,
    header::{
        ACCEPT_ENCODING, ACCEPT_RANGES, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH,
        CONTENT_RANGE, CONTENT_TYPE, ETAG, HeaderName, VARY,
    },
    http::HeaderValue,
};
use r3v3rs3_api::compression::{Compression, CompressionAlgorithm, smart_min_size_for_mime};
use r3v3rs3_api::compression_dictionary::CompressionDictionary;
use std::{io, pin::Pin, sync::Arc};
use tokio::io::AsyncRead;
use tokio_util::io::{ReaderStream, StreamReader};

/// Brotli quality 11, the library default, is too slow for streamed responses.
const BROTLI_QUALITY: i32 = 4;

const AVAILABLE_DICTIONARY: HeaderName = HeaderName::from_static("available-dictionary");
const SEC_AVAILABLE_DICTIONARY: HeaderName = HeaderName::from_static("sec-available-dictionary");
const CONTENT_DICTIONARY: HeaderName = HeaderName::from_static("content-dictionary");

/// The compression settings of a route and the encoding that the client accepts.
#[derive(Debug)]
pub struct ResponseCompression {
    config: Arc<Compression>,
    algorithm: Option<CompressionAlgorithm>,
    matched_dictionary: Option<Arc<CompressionDictionary>>,
    head: bool,
}

impl ResponseCompression {
    pub fn new(config: Arc<Compression>, method: &Method, path: &str, headers: &HeaderMap) -> Self {
        let (algorithm, matched_dictionary) = match_dictionary_or_standard(&config, path, headers);
        Self {
            config,
            algorithm,
            matched_dictionary,
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
        append_vary(res.headers_mut(), self.matched_dictionary.is_some());
        let Some(algorithm) = self.algorithm else {
            return res;
        };
        let level = resolve_level(&self.config, algorithm, res.headers());
        set_encoded_headers(
            res.headers_mut(),
            algorithm,
            self.matched_dictionary.as_deref(),
        );
        if self.head {
            return res;
        }
        if let Some(ref dict) = self.matched_dictionary {
            let is_brotli = algorithm == CompressionAlgorithm::Dcb;
            let lvl = level_to_i32(level, is_brotli);
            res.map(|body| encode_dictionary_body(body, algorithm, dict.clone(), lvl))
        } else {
            res.map(|body| encode_body(body, algorithm, level))
        }
    }
}

fn match_dictionary_or_standard(
    config: &Compression,
    path: &str,
    headers: &HeaderMap,
) -> (
    Option<CompressionAlgorithm>,
    Option<Arc<CompressionDictionary>>,
) {
    if !config.dictionaries.is_empty()
        && let Some((dict, algo)) = negotiate_dictionary(config, path, headers)
    {
        return (Some(algo), Some(dict));
    }
    (negotiate(&config.algorithms, headers), None)
}

fn client_available_dictionary(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AVAILABLE_DICTIONARY)
        .or_else(|| headers.get(SEC_AVAILABLE_DICTIONARY))
        .and_then(|val| val.to_str().ok())
        .map(str::trim)
}

fn negotiate_dictionary(
    config: &Compression,
    path: &str,
    headers: &HeaderMap,
) -> Option<(Arc<CompressionDictionary>, CompressionAlgorithm)> {
    let client_hash = client_available_dictionary(headers)?;
    let clean_client_hash = client_hash.trim_matches(':');
    let dict = config.dictionaries.iter().find(|d| {
        let dict_hash = if d.sha256_sf.is_empty() {
            CompressionDictionary::calculate_sha256_sf(d.raw_data.as_bytes())
        } else {
            d.sha256_sf.clone()
        };
        d.matches_path(path) && dict_hash.trim_matches(':') == clean_client_hash
    })?;

    let accepted = accepted_codings(headers);
    let q_dcz = accepted
        .iter()
        .find(|(name, _)| name == "dcz")
        .map_or(0, |(_, q)| *q);
    let q_dcb = accepted
        .iter()
        .find(|(name, _)| name == "dcb")
        .map_or(0, |(_, q)| *q);

    if q_dcz > 0 && q_dcz >= q_dcb {
        Some((Arc::new(dict.clone()), CompressionAlgorithm::Dcz))
    } else if q_dcb > 0 {
        Some((Arc::new(dict.clone()), CompressionAlgorithm::Dcb))
    } else {
        None
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
    let min_size = if config.adaptive_level {
        headers
            .get(CONTENT_TYPE)
            .and_then(|val| val.to_str().ok())
            .map_or(config.min_size, |ct| {
                smart_min_size_for_mime(ct, config.min_size)
            })
    } else {
        config.min_size
    };
    header_number(headers, CONTENT_LENGTH).is_none_or(|length| length >= min_size)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PayloadTier {
    Small,
    Medium,
    LargeOrStream,
}

fn classify_payload(length: Option<u64>) -> PayloadTier {
    match length {
        Some(len) if len < 8 * 1024 => PayloadTier::Small,
        Some(len) if len <= 64 * 1024 => PayloadTier::Medium,
        _ => PayloadTier::LargeOrStream,
    }
}

fn adaptive_gzip_level(tier: PayloadTier) -> Level {
    match tier {
        PayloadTier::Small => Level::Best,
        PayloadTier::Medium => Level::Default,
        PayloadTier::LargeOrStream => Level::Fastest,
    }
}

fn adaptive_brotli_level(tier: PayloadTier) -> Level {
    match tier {
        PayloadTier::Small => Level::Precise(5),
        PayloadTier::Medium => Level::Precise(4),
        PayloadTier::LargeOrStream => Level::Precise(3),
    }
}

fn adaptive_zstd_level(tier: PayloadTier) -> Level {
    match tier {
        PayloadTier::Small => Level::Precise(4),
        PayloadTier::Medium => Level::Default,
        PayloadTier::LargeOrStream => Level::Fastest,
    }
}

fn default_level(algorithm: CompressionAlgorithm) -> Level {
    match algorithm {
        CompressionAlgorithm::Gzip => Level::Default,
        CompressionAlgorithm::Brotli | CompressionAlgorithm::Dcb => Level::Precise(BROTLI_QUALITY),
        CompressionAlgorithm::Zstd | CompressionAlgorithm::Dcz => Level::Default,
    }
}

fn resolve_level(
    config: &Compression,
    algorithm: CompressionAlgorithm,
    headers: &HeaderMap,
) -> Level {
    if !config.adaptive_level {
        return default_level(algorithm);
    }
    let tier = classify_payload(header_number(headers, CONTENT_LENGTH));
    match algorithm {
        CompressionAlgorithm::Gzip => adaptive_gzip_level(tier),
        CompressionAlgorithm::Brotli | CompressionAlgorithm::Dcb => adaptive_brotli_level(tier),
        CompressionAlgorithm::Zstd | CompressionAlgorithm::Dcz => adaptive_zstd_level(tier),
    }
}

fn level_to_i32(level: Level, is_brotli: bool) -> i32 {
    match level {
        Level::Fastest => 1,
        Level::Default if is_brotli => BROTLI_QUALITY,
        Level::Default => 3,
        Level::Best if is_brotli => 6,
        Level::Best => 7,
        Level::Precise(p) => p,
        _ => {
            if is_brotli {
                BROTLI_QUALITY
            } else {
                3
            }
        }
    }
}

/// Returns the value of a header that holds a non-negative integer.
pub(super) fn header_number(headers: &HeaderMap, name: hyper::header::HeaderName) -> Option<u64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
}

/// Returns the trimmed items of a comma-separated header.
pub(super) fn header_items(
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

fn append_vary(headers: &mut HeaderMap, has_dictionary: bool) {
    let vary_accept = header_items(headers, VARY)
        .any(|name| name == "*" || name.eq_ignore_ascii_case("accept-encoding"));
    if !vary_accept {
        headers.append(VARY, HeaderValue::from_static("Accept-Encoding"));
    }
    if has_dictionary {
        let vary_dict = header_items(headers, VARY)
            .any(|name| name == "*" || name.eq_ignore_ascii_case("available-dictionary"));
        if !vary_dict {
            headers.append(VARY, HeaderValue::from_static("Available-Dictionary"));
        }
    }
}

fn set_encoded_headers(
    headers: &mut HeaderMap,
    algorithm: CompressionAlgorithm,
    dict: Option<&CompressionDictionary>,
) {
    headers.remove(CONTENT_LENGTH);
    headers.remove(ACCEPT_RANGES);
    headers.insert(
        CONTENT_ENCODING,
        HeaderValue::from_static(algorithm.as_str()),
    );
    if let Some(d) = dict {
        let hash_sf = if d.sha256_sf.is_empty() {
            CompressionDictionary::calculate_sha256_sf(d.raw_data.as_bytes())
        } else {
            d.sha256_sf.clone()
        };
        if let Ok(val) = HeaderValue::from_str(&hash_sf) {
            headers.insert(CONTENT_DICTIONARY, val);
        }
    }
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
    level: Level,
) -> BoxBody<Bytes, anyhow::Error> {
    let reader = StreamReader::new(body.into_data_stream().map_err(io::Error::other));
    let encoder: Pin<Box<dyn AsyncRead + Send + Sync>> = match algorithm {
        CompressionAlgorithm::Gzip => Box::pin(GzipEncoder::with_quality(reader, level)),
        CompressionAlgorithm::Brotli => Box::pin(BrotliEncoder::with_quality(reader, level)),
        CompressionAlgorithm::Zstd => Box::pin(ZstdEncoder::with_quality(reader, level)),
        CompressionAlgorithm::Dcb | CompressionAlgorithm::Dcz => {
            Box::pin(ZstdEncoder::with_quality(reader, level))
        }
    };
    let frames = ReaderStream::new(encoder)
        .map_ok(Frame::data)
        .map_err(anyhow::Error::from);
    BoxBody::new(StreamBody::new(frames))
}

fn compress_zstd_with_dict(
    input: &[u8],
    dict: &[u8],
    level: i32,
) -> Result<Vec<u8>, anyhow::Error> {
    let mut compressor = zstd::bulk::Compressor::with_dictionary(level, dict)
        .map_err(|e| anyhow::anyhow!("zstd dict init failed: {e}"))?;
    compressor
        .compress(input)
        .map_err(|e| anyhow::anyhow!("zstd dict compression failed: {e}"))
}

fn compress_brotli_with_dict(
    input: &[u8],
    dict: &[u8],
    quality: i32,
) -> Result<Vec<u8>, anyhow::Error> {
    let mut output = Vec::new();
    let mut input_slice = input;
    let mut input_buf = vec![0u8; 4096];
    let mut output_buf = vec![0u8; 4096];
    let params = brotli::enc::BrotliEncoderParams {
        quality,
        ..Default::default()
    };
    let alloc = brotli::enc::StandardAlloc::default();
    let mut noop = |_context: &mut brotli::enc::interface::PredictionModeContextMap<
        brotli::enc::interface::InputReferenceMut,
    >,
                    _commands: &mut [brotli::enc::interface::StaticCommand],
                    _input_pair: brotli::enc::interface::InputPair,
                    _alloc: &mut brotli::enc::StandardAlloc| {};
    brotli::BrotliCompressCustomIoCustomDict(
        &mut brotli::IoReaderWrapper(&mut input_slice),
        &mut brotli::IoWriterWrapper(&mut output),
        &mut input_buf[..],
        &mut output_buf[..],
        &params,
        alloc,
        &mut noop,
        dict,
        std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Unexpected EOF"),
    )
    .map_err(|e| anyhow::anyhow!("brotli dict compression failed: {e:?}"))?;
    Ok(output)
}

fn encode_dictionary_body(
    body: BoxBody<Bytes, anyhow::Error>,
    algorithm: CompressionAlgorithm,
    dict: Arc<CompressionDictionary>,
    level: i32,
) -> BoxBody<Bytes, anyhow::Error> {
    let stream = futures::stream::once(async move {
        let collected = body.collect().await?;
        let raw_bytes = collected.to_bytes();
        let compressed = match algorithm {
            CompressionAlgorithm::Dcz => {
                compress_zstd_with_dict(&raw_bytes, dict.raw_data.as_bytes(), level)?
            }
            CompressionAlgorithm::Dcb => {
                compress_brotli_with_dict(&raw_bytes, dict.raw_data.as_bytes(), level)?
            }
            _ => raw_bytes.to_vec(),
        };
        Ok(Frame::data(Bytes::from(compressed)))
    });
    BoxBody::new(StreamBody::new(stream))
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
            "/",
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
        let compression = ResponseCompression::new(config(), &Method::GET, "/", &HeaderMap::new());
        let res = compression.apply(response(
            "plain text body",
            &[("content-type", "text/plain")],
        ));
        assert!(!res.headers().contains_key(CONTENT_ENCODING));
        assert_eq!(res.headers()[VARY], "Accept-Encoding");
    }

    #[tokio::test]
    async fn dictionary_response_is_encoded_dcz() {
        let dict_data = "{\"schema\":\"v1\",\"users\":[".to_string();
        let mut dict = CompressionDictionary {
            id: r3v3rs3_api::id::ShortId::new(),
            name: "Users Schema".to_string(),
            match_pattern: "/api/*".to_string(),
            path: "/.well-known/dict/users.dict".to_string(),
            raw_data: dict_data.clone(),
            sha256_sf: String::new(),
        };
        dict.ensure_hash();
        let dict_hash = dict.sha256_sf.clone();

        let cfg = Arc::new(Compression {
            algorithms: vec![CompressionAlgorithm::Zstd, CompressionAlgorithm::Gzip],
            dictionaries: vec![dict],
            min_size: 10,
            ..Default::default()
        });

        let body =
            format!("{dict_data}{{\"id\":1,\"name\":\"Alice\"}},{{\"id\":2,\"name\":\"Bob\"}}]}}");
        let compression = ResponseCompression::new(
            cfg,
            &Method::GET,
            "/api/users",
            &headers(&[
                ("accept-encoding", "dcz, zstd, gzip"),
                ("available-dictionary", &dict_hash),
            ]),
        );
        let res = compression.apply(response(
            &body,
            &[
                ("content-type", "application/json"),
                ("content-length", &body.len().to_string()),
                ("etag", "\"v1\""),
            ],
        ));

        let headers = res.headers();
        assert_eq!(headers[CONTENT_ENCODING], "dcz");
        assert_eq!(headers[CONTENT_DICTIONARY], dict_hash);
        assert_eq!(headers[ETAG], "W/\"v1\"");
        let vary_items = headers
            .get_all(VARY)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect::<Vec<_>>();
        assert!(vary_items.contains(&"Accept-Encoding"));
        assert!(vary_items.contains(&"Available-Dictionary"));

        let encoded = res.into_body().collect().await.unwrap().to_bytes();
        let mut decompressor =
            zstd::bulk::Decompressor::with_dictionary(dict_data.as_bytes()).unwrap();
        let decompressed = decompressor.decompress(&encoded, 10 * 1024).unwrap();
        assert_eq!(String::from_utf8(decompressed).unwrap(), body);
    }

    #[test]
    fn payload_tier_classification() {
        assert_eq!(classify_payload(Some(1024)), PayloadTier::Small);
        assert_eq!(classify_payload(Some(7999)), PayloadTier::Small);
        assert_eq!(classify_payload(Some(8192)), PayloadTier::Medium);
        assert_eq!(classify_payload(Some(65536)), PayloadTier::Medium);
        assert_eq!(classify_payload(Some(65537)), PayloadTier::LargeOrStream);
        assert_eq!(classify_payload(None), PayloadTier::LargeOrStream);
    }

    #[test]
    fn adaptive_level_resolution() {
        let mut cfg = Compression {
            adaptive_level: true,
            ..Default::default()
        };
        let small_headers = headers(&[("content-length", "1024")]);
        let large_headers = headers(&[("content-length", "100000")]);
        let stream_headers = HeaderMap::new();

        assert!(matches!(
            resolve_level(&cfg, CompressionAlgorithm::Gzip, &small_headers),
            Level::Best
        ));
        assert!(matches!(
            resolve_level(&cfg, CompressionAlgorithm::Gzip, &large_headers),
            Level::Fastest
        ));
        assert!(matches!(
            resolve_level(&cfg, CompressionAlgorithm::Gzip, &stream_headers),
            Level::Fastest
        ));

        cfg.adaptive_level = false;
        assert!(matches!(
            resolve_level(&cfg, CompressionAlgorithm::Gzip, &small_headers),
            Level::Default
        ));
    }
}
