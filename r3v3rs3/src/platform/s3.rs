//! The S3 compatible target of the backups: one signed `PUT Object` request per backup, over
//! the same HTTPS client that the Git provider APIs use.

use crate::cdn::fetch::HttpClient;
use crate::certs::dns::sigv4::{self, SigningInput};
use anyhow::{Context, bail};
use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{Method, Request};
use http_body_util::Full;
use r3v3rs3_api::platform::{BackupEntry, S3Config};
use sha2::{Digest, Sha256};
use std::time::Duration;
use time::OffsetDateTime;

/// The time a signed upload may take.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// Uploads one backup as `{prefix}/{name}/{id}.dump.gz` under the endpoint's bucket path.
pub(crate) async fn upload(
    client: &HttpClient,
    config: &S3Config,
    entry: &BackupEntry,
    content: Vec<u8>,
) -> anyhow::Result<()> {
    let url = url::Url::parse(&config.endpoint).context("the S3 endpoint is not a URL")?;
    let scheme = url.scheme();
    if scheme != "https" && scheme != "http" {
        bail!("the S3 endpoint must be an http or https URL");
    }
    let host = host(&url);
    let path = object_path(&url, config, entry);
    let amz_date = sigv4::amz_date(OffsetDateTime::now_utc());
    let payload_hash = hex::encode(Sha256::digest(&content));
    let authorization = sigv4::authorization(
        &config.access_key,
        &config.secret_key,
        &SigningInput {
            method: "PUT",
            host: &host,
            path: &path,
            query: "",
            amz_date: &amz_date,
            payload: &content,
            region: &config.region,
            service: "s3",
        },
    )?;
    let base = format!("{scheme}://{host}");
    let request = Request::builder()
        .method(Method::PUT)
        .uri(format!("{base}{path}"))
        .header("host", &host)
        .header("x-amz-date", &amz_date)
        .header("x-amz-content-sha256", &payload_hash)
        .header("authorization", authorization)
        .header(CONTENT_TYPE, "application/octet-stream")
        .body(Full::new(Bytes::from(content)))?;
    let response = tokio::time::timeout(UPLOAD_TIMEOUT, client.request(request)).await??;
    let status = response.status();
    if !status.is_success() {
        bail!("the S3 bucket answered {status}");
    }
    Ok(())
}

/// The host header value: the host, with the port when it is not the scheme default.
fn host(url: &url::Url) -> String {
    match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    }
}

/// The URI path of one backup object: the bucket path of the endpoint, the prefix and the file.
fn object_path(url: &url::Url, config: &S3Config, entry: &BackupEntry) -> String {
    let bucket = url.path().trim_matches('/');
    let prefix = config.prefix.trim_matches('/');
    let parts = [
        bucket,
        prefix,
        &entry.name,
        &format!("{}.dump.gz", entry.id),
    ];
    let mut path = String::new();
    for part in parts {
        if !part.is_empty() {
            path.push('/');
            path.push_str(part);
        }
    }
    if path.is_empty() {
        path.push('/');
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(endpoint: &str, prefix: &str) -> S3Config {
        S3Config {
            endpoint: endpoint.to_string(),
            region: "us-east-1".to_string(),
            prefix: prefix.to_string(),
            access_key: "AKID".to_string(),
            secret_key: "secret".to_string(),
        }
    }

    fn entry() -> BackupEntry {
        BackupEntry {
            id: "1a2b".to_string(),
            app: "abc".parse().unwrap(),
            name: "shop".to_string(),
            engine: "postgres".to_string(),
            size: 4,
            sha256: "feed".to_string(),
            created_at: 1,
        }
    }

    #[test]
    fn the_object_sits_under_the_bucket_and_the_prefix() {
        let entry = entry();
        let path = object_path(
            &url::Url::parse("https://s3.example.com/bucket").unwrap(),
            &config("https://s3.example.com/bucket", "databases"),
            &entry,
        );
        assert_eq!(path, "/bucket/databases/shop/1a2b.dump.gz");
    }

    #[test]
    fn an_empty_prefix_and_bucket_still_give_a_key() {
        let entry = entry();
        let path = object_path(
            &url::Url::parse("https://s3.example.com").unwrap(),
            &config("https://s3.example.com", "/"),
            &entry,
        );
        assert_eq!(path, "/shop/1a2b.dump.gz");
    }

    #[test]
    fn the_host_carries_a_non_default_port() {
        assert_eq!(
            host(&url::Url::parse("http://127.0.0.1:9000").unwrap()),
            "127.0.0.1:9000"
        );
        assert_eq!(
            host(&url::Url::parse("https://s3.example.com").unwrap()),
            "s3.example.com"
        );
    }
}
