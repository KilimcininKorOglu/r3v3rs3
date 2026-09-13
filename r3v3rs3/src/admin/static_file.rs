use axum::{
    body::Bytes,
    http::{
        header::{CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, ETAG, IF_NONE_MATCH},
        HeaderMap, HeaderValue, StatusCode, Uri,
    },
    response::IntoResponse,
};
use fnv::FnvHasher;
use include_dir::{include_dir, Dir};
use std::{hash::Hasher, path::Path};

use super::AppError;

static STATIC_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/dist");

const IMMUTABLE_FILE_PREFIXES: &[&str] = &["r3v3rs3-webui-", "tailwind-"];

pub async fn fallback(uri: Uri, req_headers: HeaderMap) -> Result<impl IntoResponse, AppError> {
    let path = uri.path();
    if path.starts_with("/api/") {
        return Err(AppError::NotFound);
    }
    let file_name = requested_file_name(path);

    let Some(file) = STATIC_DIR.get_file(Path::new("webui").join(format!("{file_name}.gz"))) else {
        return Err(AppError::NotFound);
    };

    let mut hasher = FnvHasher::default();
    hasher.write(file.contents());
    let etag = format!("{:x}", hasher.finish());

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static(cache_control(file_name)),
    );
    headers.insert(ETAG, header_value(&etag)?);

    if req_headers
        .get(IF_NONE_MATCH)
        .map(|x| x.to_str().unwrap_or_default())
        == Some(&etag)
    {
        return Ok((StatusCode::NOT_MODIFIED, headers, Bytes::new()));
    }

    let ext = file
        .path()
        .file_stem()
        .and_then(|x| x.to_str())
        .unwrap_or_default();
    let mime = mime_guess::from_path(ext).first_or_octet_stream();
    headers.insert(CONTENT_TYPE, header_value(mime.as_ref())?);

    Ok((StatusCode::OK, headers, Bytes::from_static(file.contents())))
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

    #[tokio::test]
    async fn api_paths_are_not_served() {
        let uri: Uri = "/api/unknown".parse().unwrap();
        assert!(fallback(uri, HeaderMap::new()).await.is_err());
    }
}
