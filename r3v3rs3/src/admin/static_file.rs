use axum::{
    body::Bytes,
    extract::{RawQuery, State},
    http::{
        HeaderMap, HeaderValue, StatusCode, Uri,
        header::{
            ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, ETAG, IF_NONE_MATCH,
            VARY,
        },
    },
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::CookieJar;
use flate2::read::GzDecoder;
use fnv::FnvHasher;
use include_dir::{Dir, include_dir};
use percent_encoding::utf8_percent_encode;
use std::{collections::HashMap, hash::Hasher, io::Read, path::Path, sync::LazyLock};

use super::{AppError, AppState};

static STATIC_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/dist");

const IMMUTABLE_FILE_PREFIXES: &[&str] = &["r3v3rs3-webui-", "tailwind-"];

/// The content hash of every embedded file, computed once. The files are compiled into the binary,
/// so a hash never changes while the process runs.
static ETAGS: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    let mut etags = HashMap::new();
    let mut dirs = vec![&STATIC_DIR];
    while let Some(dir) = dirs.pop() {
        for file in dir.files() {
            let mut hasher = FnvHasher::default();
            hasher.write(file.contents());
            etags.insert(
                file.path().to_string_lossy().into_owned(),
                format!("{:x}", hasher.finish()),
            );
        }
        dirs.extend(dir.dirs());
    }
    etags
});

pub async fn fallback(
    State(state): State<AppState>,
    jar: CookieJar,
    RawQuery(query): RawQuery,
    uri: Uri,
    req_headers: HeaderMap,
) -> Result<Response, AppError> {
    let path = uri.path();
    if path.starts_with("/api/") {
        return Err(AppError::NotFound);
    }
    let file_name = requested_file_name(path);
    if is_webui_shell(path) && !is_signed_in(&state, &jar).await {
        // The shell of a WebUI route reaches the browser only with a session, so a
        // signed-out navigation never renders a page, not for a moment.
        return Ok(Redirect::to(&login_target(path, query.as_deref())).into_response());
    }
    serve_file(file_name, &req_headers).await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StaticEncoding {
    Zstd,
    Brotli,
    Gzip,
}

impl StaticEncoding {
    fn extension(self) -> &'static str {
        match self {
            Self::Zstd => "zst",
            Self::Brotli => "br",
            Self::Gzip => "gz",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Zstd => "zstd",
            Self::Brotli => "br",
            Self::Gzip => "gzip",
        }
    }
}

struct SelectedFile<'a> {
    file: &'a include_dir::File<'a>,
    file_path: std::path::PathBuf,
    encoding: Option<StaticEncoding>,
}

fn find_client_encoding<'a>(file_name: &str, req_headers: &HeaderMap) -> Option<SelectedFile<'a>> {
    let candidates = [
        (StaticEncoding::Zstd, accepts_encoding(req_headers, "zstd")),
        (StaticEncoding::Brotli, accepts_encoding(req_headers, "br")),
        (StaticEncoding::Gzip, accepts_encoding(req_headers, "gzip")),
    ];
    for (enc, accepted) in candidates {
        if accepted {
            let path = Path::new("webui").join(format!("{file_name}.{}", enc.extension()));
            if let Some(file) = STATIC_DIR.get_file(&path) {
                return Some(SelectedFile {
                    file,
                    file_path: path,
                    encoding: Some(enc),
                });
            }
        }
    }
    None
}

fn find_identity_fallback<'a>(file_name: &str) -> Option<SelectedFile<'a>> {
    let raw_path = Path::new("webui").join(file_name);
    if let Some(file) = STATIC_DIR.get_file(&raw_path) {
        return Some(SelectedFile {
            file,
            file_path: raw_path,
            encoding: None,
        });
    }
    for enc in [
        StaticEncoding::Gzip,
        StaticEncoding::Brotli,
        StaticEncoding::Zstd,
    ] {
        let path = Path::new("webui").join(format!("{file_name}.{}", enc.extension()));
        if let Some(file) = STATIC_DIR.get_file(&path) {
            return Some(SelectedFile {
                file,
                file_path: path,
                encoding: None,
            });
        }
    }
    None
}

fn find_precompressed<'a>(file_name: &str, req_headers: &HeaderMap) -> Option<SelectedFile<'a>> {
    find_client_encoding(file_name, req_headers).or_else(|| find_identity_fallback(file_name))
}

/// Answers a file of the embedded WebUI bundle.
async fn serve_file(file_name: &str, req_headers: &HeaderMap) -> Result<Response, AppError> {
    let selected = find_precompressed(file_name, req_headers).ok_or(AppError::NotFound)?;

    let hash = ETAGS
        .get(selected.file_path.to_string_lossy().as_ref())
        .ok_or(AppError::NotFound)?;

    let mut headers = HeaderMap::new();
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static(cache_control(file_name)),
    );
    headers.insert(VARY, HeaderValue::from_static("Accept-Encoding"));
    if let Some(enc) = selected.encoding {
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static(enc.as_str()));
    }

    if let Some(etag) = validator_with_encoding(file_name, hash, selected.encoding) {
        headers.insert(ETAG, header_value(&etag)?);
        if matches_etag(req_headers, &etag) {
            return Ok((StatusCode::NOT_MODIFIED, headers, Bytes::new()).into_response());
        }
    }

    let mime = mime_guess::from_path(file_name).first_or_octet_stream();
    headers.insert(CONTENT_TYPE, header_value(mime.as_ref())?);

    let body = match selected.encoding {
        Some(_) => Bytes::from_static(selected.file.contents()),
        None => Bytes::from(decompress_to_identity(
            &selected.file_path,
            selected.file.contents(),
        )?),
    };
    Ok((StatusCode::OK, headers, body).into_response())
}

/// A WebUI route loads the SPA shell. The sign-in page is the only shell a signed-out
/// client may load, and the hashed bundle files are not shells.
fn is_webui_shell(path: &str) -> bool {
    requested_file_name(path) == "index.html" && path != "/login"
}

/// Checks the session cookie the way the admin API does.
async fn is_signed_in(state: &AppState, jar: &CookieJar) -> bool {
    match jar.get("token").map(|cookie| cookie.value()) {
        Some(token) => super::auth::session_caller(state, token).await.is_ok(),
        None => false,
    }
}

/// The sign-in target that returns the client to the page it asked for.
fn login_target(path: &str, query: Option<&str>) -> String {
    let target = match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    if target == "/" {
        return "/login".to_string();
    }
    format!(
        "/login?redirect={}",
        utf8_percent_encode(&target, percent_encoding::NON_ALPHANUMERIC)
    )
}

/// The ETag of a file that a browser revalidates. A hashed bundle file is immutable, so the
/// browser never revalidates it and it gets no ETag.
#[cfg(test)]
fn validator(file_name: &str, hash: &str, gzip: bool) -> Option<String> {
    (!is_immutable(file_name)).then(|| etag_of(hash, gzip))
}

fn validator_with_encoding(
    file_name: &str,
    hash: &str,
    encoding: Option<StaticEncoding>,
) -> Option<String> {
    (!is_immutable(file_name)).then(|| etag_with_encoding(hash, encoding))
}

/// The gzip body and the plain body are different bytes, so each one gets its own strong ETag.
#[cfg(test)]
fn etag_of(hash: &str, gzip: bool) -> String {
    if gzip {
        format!("\"{hash}\"")
    } else {
        format!("\"{hash}-identity\"")
    }
}

fn etag_with_encoding(hash: &str, encoding: Option<StaticEncoding>) -> String {
    match encoding {
        Some(StaticEncoding::Gzip) => format!("\"{hash}\""),
        Some(enc) => format!("\"{hash}-{}\"", enc.as_str()),
        None => format!("\"{hash}-identity\""),
    }
}

/// A client that does not list gzip gets the plain body. An explicit `gzip;q=0` refuses gzip.
#[cfg(test)]
fn accepts_gzip(headers: &HeaderMap) -> bool {
    accepts_encoding(headers, "gzip")
}

fn accepts_encoding(headers: &HeaderMap, coding: &str) -> bool {
    let Some(value) = headers.get(ACCEPT_ENCODING).and_then(|x| x.to_str().ok()) else {
        return false;
    };
    value.split(',').any(|entry| {
        let mut parts = entry.split(';');
        let name = parts.next().unwrap_or_default().trim();
        (name.eq_ignore_ascii_case(coding) || name == "*") && !parts.any(is_zero_quality)
    })
}

/// A `q` parameter whose value is zero, such as `q=0` or `q=0.000`, refuses the coding
/// (RFC 9110 section 12.4.2).
fn is_zero_quality(param: &str) -> bool {
    let param = param.trim().replace(' ', "").to_ascii_lowercase();
    param
        .strip_prefix("q=")
        .and_then(|value| value.parse::<f32>().ok())
        == Some(0.0)
}

/// Compares the response ETag with `If-None-Match`, which holds a list, `*`, or a weak validator
/// (RFC 9110 section 13.1.2).
fn matches_etag(headers: &HeaderMap, etag: &str) -> bool {
    let Some(value) = headers.get(IF_NONE_MATCH).and_then(|x| x.to_str().ok()) else {
        return false;
    };
    if value.trim() == "*" {
        return true;
    }
    let strip_weak = |tag: &str| tag.trim().trim_start_matches("W/").to_string();
    let candidate = strip_weak(etag);
    value.split(',').any(|tag| strip_weak(tag) == candidate)
}

fn gunzip(data: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut out = Vec::new();
    GzDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|err| AppError::Anyhow(err.into()))?;
    Ok(out)
}

fn brotli_decompress(data: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut out = Vec::new();
    let mut reader = brotli::Decompressor::new(data, 4096);
    reader
        .read_to_end(&mut out)
        .map_err(|err| AppError::Anyhow(err.into()))?;
    Ok(out)
}

fn zstd_decompress(data: &[u8]) -> Result<Vec<u8>, AppError> {
    zstd::decode_all(data).map_err(|err| AppError::Anyhow(err.into()))
}

fn decompress_to_identity(file_path: &Path, contents: &[u8]) -> Result<Vec<u8>, AppError> {
    let ext = file_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    match ext {
        "gz" => gunzip(contents),
        "br" => brotli_decompress(contents),
        "zst" => zstd_decompress(contents),
        _ => Ok(contents.to_vec()),
    }
}

/// Paths without a file extension are WebUI routes, so they load index.html.
fn requested_file_name(path: &str) -> &str {
    let path_has_extension = path
        .rfind('.')
        .map(|i| i > path.rfind('/').unwrap_or(0))
        .unwrap_or_default();
    if path == "/" || !path_has_extension {
        "index.html"
    } else {
        path.trim_start_matches('/')
    }
}

/// Hashed bundle files never change, so browsers may keep them. Every other file,
/// index.html included, must be revalidated so that an upgrade loads the new bundle.
fn cache_control(file_name: &str) -> &'static str {
    if is_immutable(file_name) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

fn is_immutable(file_name: &str) -> bool {
    IMMUTABLE_FILE_PREFIXES
        .iter()
        .any(|prefix| file_name.starts_with(prefix))
}

fn header_value(value: &str) -> Result<HeaderValue, AppError> {
    HeaderValue::from_str(value).map_err(|err| AppError::Anyhow(err.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests do not read STATIC_DIR, because CI builds the server without the WebUI bundle.
    fn cache_control_of(path: &str) -> &'static str {
        cache_control(requested_file_name(path))
    }

    fn headers(name: axum::http::HeaderName, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(name, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn index_html_is_revalidated() {
        assert_eq!(requested_file_name("/"), "index.html");
        assert_eq!(requested_file_name("/proxies/abc"), "index.html");
        assert_eq!(requested_file_name("/proxies/a.b/edit"), "index.html");
        assert_eq!(cache_control_of("/"), "no-cache");
        assert_eq!(cache_control_of("/proxies/abc"), "no-cache");
        assert_eq!(cache_control_of("/robots.txt"), "no-cache");
    }

    #[test]
    fn hashed_bundle_is_immutable() {
        for path in [
            "/r3v3rs3-webui-f5ed843114223c55.js",
            "/r3v3rs3-webui-f5ed843114223c55_bg.wasm",
            "/tailwind-cd802f743c089f72.css",
        ] {
            assert_eq!(
                cache_control_of(path),
                "public, max-age=31536000, immutable"
            );
        }
    }

    #[test]
    fn etag_is_quoted_and_differs_per_encoding() {
        assert_eq!(etag_of("abc", true), "\"abc\"");
        assert_eq!(etag_of("abc", false), "\"abc-identity\"");
        assert_eq!(
            etag_with_encoding("abc", Some(StaticEncoding::Zstd)),
            "\"abc-zstd\""
        );
        assert_eq!(
            etag_with_encoding("abc", Some(StaticEncoding::Brotli)),
            "\"abc-br\""
        );
        assert_eq!(
            etag_with_encoding("abc", Some(StaticEncoding::Gzip)),
            "\"abc\""
        );
        assert_eq!(etag_with_encoding("abc", None), "\"abc-identity\"");
    }

    #[test]
    fn only_revalidated_files_get_an_etag() {
        assert_eq!(validator("index.html", "abc", true), Some("\"abc\"".into()));
        assert_eq!(
            validator("robots.txt", "abc", false),
            Some("\"abc-identity\"".into())
        );
        assert_eq!(
            validator("r3v3rs3-webui-f5ed843114223c55.js", "abc", true),
            None
        );
        assert_eq!(
            validator("tailwind-cd802f743c089f72.css", "abc", true),
            None
        );
    }

    #[test]
    fn if_none_match_accepts_a_list_a_wildcard_and_a_weak_tag() {
        let etag = "\"abc\"";
        assert!(matches_etag(&headers(IF_NONE_MATCH, "\"abc\""), etag));
        assert!(matches_etag(
            &headers(IF_NONE_MATCH, "\"other\", \"abc\", \"more\""),
            etag
        ));
        assert!(matches_etag(&headers(IF_NONE_MATCH, "W/\"abc\""), etag));
        assert!(matches_etag(&headers(IF_NONE_MATCH, "*"), etag));
        assert!(!matches_etag(&headers(IF_NONE_MATCH, "\"other\""), etag));
        assert!(!matches_etag(&HeaderMap::new(), etag));
    }

    #[test]
    fn gzip_is_sent_only_to_a_client_that_accepts_it() {
        assert!(accepts_gzip(&headers(ACCEPT_ENCODING, "gzip, deflate, br")));
        assert!(accepts_gzip(&headers(ACCEPT_ENCODING, "*")));
        assert!(!accepts_gzip(&headers(ACCEPT_ENCODING, "br, deflate")));
        assert!(!accepts_gzip(&headers(ACCEPT_ENCODING, "gzip;q=0")));
        assert!(!accepts_gzip(&headers(ACCEPT_ENCODING, "gzip; q=0.0")));
        assert!(!accepts_gzip(&headers(ACCEPT_ENCODING, "br, gzip;Q=0.000")));
        assert!(accepts_gzip(&headers(ACCEPT_ENCODING, "GZIP;q=0.5")));
        assert!(!accepts_gzip(&headers(ACCEPT_ENCODING, "identity")));
        assert!(!accepts_gzip(&HeaderMap::new()));
        assert!(accepts_encoding(
            &headers(ACCEPT_ENCODING, "br, gzip"),
            "br"
        ));
        assert!(accepts_encoding(
            &headers(ACCEPT_ENCODING, "zstd;q=1.0"),
            "zstd"
        ));
        assert!(!accepts_encoding(
            &headers(ACCEPT_ENCODING, "zstd;q=0"),
            "zstd"
        ));
    }

    #[test]
    fn a_webui_route_is_a_shell_and_the_sign_in_page_is_not() {
        for path in ["/", "/ports", "/proxies/abc", "/proxies/a.b/edit"] {
            assert!(is_webui_shell(path), "{path}");
        }
        for path in [
            "/login",
            "/r3v3rs3-webui-f5ed843114223c55.js",
            "/tailwind-cd802f743c089f72.css",
            "/robots.txt",
        ] {
            assert!(!is_webui_shell(path), "{path}");
        }
    }

    #[test]
    fn the_login_target_keeps_the_requested_page() {
        assert_eq!(login_target("/", None), "/login");
        assert_eq!(login_target("/ports", None), "/login?redirect=%2Fports");
        assert_eq!(
            login_target("/ports", Some("a=1")),
            "/login?redirect=%2Fports%3Fa%3D1"
        );
    }
}
