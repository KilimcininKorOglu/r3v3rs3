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
    let path_has_extension = path
        .rfind('.')
        .map(|i| i > path.rfind('/').unwrap_or(0))
        .unwrap_or_default();
    let file_name = if path == "/" || !path_has_extension {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };

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

    async fn cache_control_of(path: &str) -> String {
        let uri: Uri = path.parse().unwrap();
        let Ok(response) = fallback(uri, HeaderMap::new()).await else {
            panic!("static file not found: {path}");
        };
        response.into_response().headers()[CACHE_CONTROL]
            .to_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn index_html_is_revalidated() {
        assert_eq!(cache_control_of("/").await, "no-cache");
        assert_eq!(cache_control_of("/proxies/abc").await, "no-cache");
    }

    #[tokio::test]
    async fn hashed_bundle_is_immutable() {
        let bundle = STATIC_DIR
            .get_dir("webui")
            .and_then(|dir| {
                dir.files()
                    .filter_map(|file| file.path().file_name()?.to_str())
                    .find(|name| name.starts_with("r3v3rs3-webui-") && name.ends_with(".js.gz"))
            })
            .unwrap();
        let path = format!("/{}", bundle.trim_end_matches(".gz"));
        assert_eq!(
            cache_control_of(&path).await,
            "public, max-age=31536000, immutable"
        );
    }
}
