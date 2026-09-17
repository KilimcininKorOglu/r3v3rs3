use axum::{
    body::Bytes,
    http::{
        HeaderMap, HeaderValue, StatusCode, Uri,
        header::{
            ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, ETAG, IF_NONE_MATCH,
            VARY,
        },
    },
    response::IntoResponse,
};
use flate2::read::GzDecoder;
use fnv::FnvHasher;
use include_dir::{Dir, include_dir};
use std::{collections::HashMap, hash::Hasher, io::Read, path::Path, sync::LazyLock};

use super::AppError;

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

pub async fn fallback(uri: Uri, req_headers: HeaderMap) -> Result<impl IntoResponse, AppError> {
    let path = uri.path();
    if path.starts_with("/api/") {
        return Err(AppError::NotFound);
    }
    let file_name = requested_file_name(path);
    let file_path = Path::new("webui").join(format!("{file_name}.gz"));

    let Some(file) = STATIC_DIR.get_file(&file_path) else {
        return Err(AppError::NotFound);
    };

    let hash = ETAGS
        .get(file_path.to_string_lossy().as_ref())
        .ok_or(AppError::NotFound)?;
    let gzip = accepts_gzip(&req_headers);
    let etag = etag_of(hash, gzip);

    let mut headers = HeaderMap::new();
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static(cache_control(file_name)),
    );
    headers.insert(ETAG, header_value(&etag)?);
    // The stored file is gzip encoded, so the body differs per Accept-Encoding.
    headers.insert(VARY, HeaderValue::from_static("Accept-Encoding"));
    if gzip {
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }

    if matches_etag(&req_headers, &etag) {
        return Ok((StatusCode::NOT_MODIFIED, headers, Bytes::new()));
    }

    let ext = file
        .path()
        .file_stem()
        .and_then(|x| x.to_str())
        .unwrap_or_default();
    let mime = mime_guess::from_path(ext).first_or_octet_stream();
    headers.insert(CONTENT_TYPE, header_value(mime.as_ref())?);

    let body = if gzip {
        Bytes::from_static(file.contents())
    } else {
        Bytes::from(gunzip(file.contents())?)
    };
    Ok((StatusCode::OK, headers, body))
}

/// The gzip body and the plain body are different bytes, so each one gets its own strong ETag.
fn etag_of(hash: &str, gzip: bool) -> String {
    if gzip {
        format!("\"{hash}\"")
    } else {
        format!("\"{hash}-identity\"")
    }
}

/// A client that does not list gzip gets the plain body. An explicit `gzip;q=0` refuses gzip.
fn accepts_gzip(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(ACCEPT_ENCODING).and_then(|x| x.to_str().ok()) else {
        return false;
    };
    value.split(',').any(|entry| {
        let mut parts = entry.split(';');
        let coding = parts.next().unwrap_or_default().trim();
        if coding != "gzip" && coding != "*" {
            return false;
        }
        !parts.any(|param| param.trim().replace(' ', "") == "q=0")
    })
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
    if IMMUTABLE_FILE_PREFIXES
        .iter()
        .any(|prefix| file_name.starts_with(prefix))
    {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
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
        assert!(!accepts_gzip(&headers(ACCEPT_ENCODING, "identity")));
        assert!(!accepts_gzip(&HeaderMap::new()));
    }

    #[tokio::test]
    async fn api_paths_are_not_served() {
        let uri: Uri = "/api/unknown".parse().unwrap();
        assert!(fallback(uri, HeaderMap::new()).await.is_err());
    }
}
