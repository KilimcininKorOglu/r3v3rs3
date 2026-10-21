//! The S3 compatible target of the backups: signed PUT/GET/List requests over
//! the same HTTPS client that the Git provider APIs use.

use crate::cdn::fetch::HttpClient;
use crate::certs::dns::sigv4::{self, SigningInput};
use anyhow::{Context, bail};
use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{Method, Request};
use http_body_util::{BodyExt, Full};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use r3v3rs3_api::platform::{BackupEntry, S3Config};
use sha2::{Digest, Sha256};
use std::time::Duration;
use time::OffsetDateTime;

/// The time a signed request may take.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// Characters preserved in AWS SigV4 query strings.
const AWS_QUERY_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// An S3 object entry returned from `ListObjectsV2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct S3ObjectItem {
    pub key: String,
    pub size: u64,
    pub last_modified: String,
}

/// Uploads one backup as `{prefix}/{name}/{id}.dump.gz` under the endpoint's bucket path.
pub(crate) async fn upload(
    client: &HttpClient,
    config: &S3Config,
    entry: &BackupEntry,
    content: Vec<u8>,
) -> anyhow::Result<()> {
    let key = format!("{}/{}.dump.gz", entry.name, entry.id);
    upload_key(client, config, &key, content).await
}

/// Uploads content under `{prefix}/{key}` to the configured S3 endpoint.
pub(crate) async fn upload_key(
    client: &HttpClient,
    config: &S3Config,
    key: &str,
    content: Vec<u8>,
) -> anyhow::Result<()> {
    let url = url::Url::parse(&config.endpoint).context("the S3 endpoint is not a URL")?;
    let scheme = url.scheme();
    if scheme != "https" && scheme != "http" {
        bail!("the S3 endpoint must be an http or https URL");
    }
    let host = host(&url);
    let path = object_key_path(&url, config, key);

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

/// Downloads the content of `{prefix}/{key}` from the configured S3 endpoint.
pub(crate) async fn download_key(
    client: &HttpClient,
    config: &S3Config,
    key: &str,
) -> anyhow::Result<Vec<u8>> {
    let url = url::Url::parse(&config.endpoint).context("the S3 endpoint is not a URL")?;
    let scheme = url.scheme();
    if scheme != "https" && scheme != "http" {
        bail!("the S3 endpoint must be an http or https URL");
    }
    let host = host(&url);
    let path = object_key_path(&url, config, key);

    let amz_date = sigv4::amz_date(OffsetDateTime::now_utc());
    let empty_payload: &[u8] = b"";
    let payload_hash = hex::encode(Sha256::digest(empty_payload));
    let authorization = sigv4::authorization(
        &config.access_key,
        &config.secret_key,
        &SigningInput {
            method: "GET",
            host: &host,
            path: &path,
            query: "",
            amz_date: &amz_date,
            payload: empty_payload,
            region: &config.region,
            service: "s3",
        },
    )?;
    let base = format!("{scheme}://{host}");
    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("{base}{path}"))
        .header("host", &host)
        .header("x-amz-date", &amz_date)
        .header("x-amz-content-sha256", &payload_hash)
        .header("authorization", authorization)
        .body(Full::new(Bytes::new()))?;
    let response = tokio::time::timeout(UPLOAD_TIMEOUT, client.request(request)).await??;
    let status = response.status();
    if !status.is_success() {
        bail!("the S3 bucket answered {status}");
    }
    let body = BodyExt::collect(response.into_body())
        .await
        .map_err(|err| anyhow::anyhow!("{err}"))?
        .to_bytes()
        .to_vec();
    Ok(body)
}

fn build_list_request(
    config: &S3Config,
    filter_prefix: &str,
) -> anyhow::Result<Request<Full<Bytes>>> {
    let url = url::Url::parse(&config.endpoint).context("the S3 endpoint is not a URL")?;
    let scheme = url.scheme();
    if scheme != "https" && scheme != "http" {
        bail!("the S3 endpoint must be an http or https URL");
    }
    let host = host(&url);
    let path = bucket_path(&url);
    let query = list_query_string(config, filter_prefix);

    let amz_date = sigv4::amz_date(OffsetDateTime::now_utc());
    let empty_payload: &[u8] = b"";
    let payload_hash = hex::encode(Sha256::digest(empty_payload));
    let authorization = sigv4::authorization(
        &config.access_key,
        &config.secret_key,
        &SigningInput {
            method: "GET",
            host: &host,
            path: &path,
            query: &query,
            amz_date: &amz_date,
            payload: empty_payload,
            region: &config.region,
            service: "s3",
        },
    )?;
    let base = format!("{scheme}://{host}");
    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("{base}{path}?{query}"))
        .header("host", &host)
        .header("x-amz-date", &amz_date)
        .header("x-amz-content-sha256", &payload_hash)
        .header("authorization", authorization)
        .body(Full::new(Bytes::new()))?;
    Ok(request)
}

/// Lists objects under `{prefix}/{filter_prefix}` using S3 `ListObjectsV2`.
pub(crate) async fn list_objects(
    client: &HttpClient,
    config: &S3Config,
    filter_prefix: &str,
) -> anyhow::Result<Vec<S3ObjectItem>> {
    let request = build_list_request(config, filter_prefix)?;
    let response = tokio::time::timeout(UPLOAD_TIMEOUT, client.request(request)).await??;
    if !response.status().is_success() {
        bail!("the S3 bucket answered {}", response.status());
    }
    let body_bytes = BodyExt::collect(response.into_body())
        .await
        .map_err(|err| anyhow::anyhow!("{err}"))?
        .to_bytes();
    let text = std::str::from_utf8(&body_bytes).context("S3 response is not valid UTF-8")?;
    Ok(parse_s3_contents(text))
}

fn list_query_string(config: &S3Config, filter_prefix: &str) -> String {
    let p = list_query_prefix(config, filter_prefix);
    if p.is_empty() {
        "list-type=2".to_string()
    } else {
        let encoded = utf8_percent_encode(&p, AWS_QUERY_SET).to_string();
        format!("list-type=2&prefix={encoded}")
    }
}

fn list_query_prefix(config: &S3Config, filter: &str) -> String {
    let prefix = config.prefix.trim_matches('/');
    let filter = filter.trim_matches('/');
    match (prefix.is_empty(), filter.is_empty()) {
        (true, true) => String::new(),
        (false, true) => prefix.to_string(),
        (true, false) => filter.to_string(),
        (false, false) => format!("{prefix}/{filter}"),
    }
}

fn bucket_path(url: &url::Url) -> String {
    let bucket = url.path().trim_matches('/');
    if bucket.is_empty() {
        "/".to_string()
    } else {
        format!("/{bucket}")
    }
}

/// The host header value: the host, with the port when it is not the scheme default.
fn host(url: &url::Url) -> String {
    match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    }
}

/// The URI path of one object: the bucket path of the endpoint, the prefix and the key.
fn object_key_path(url: &url::Url, config: &S3Config, key: &str) -> String {
    let bucket = url.path().trim_matches('/');
    let prefix = config.prefix.trim_matches('/');
    let mut clean_key = key.trim_start_matches('/');
    if !prefix.is_empty() && clean_key.starts_with(prefix) {
        clean_key = clean_key[prefix.len()..].trim_start_matches('/');
    }
    let parts = [bucket, prefix, clean_key];
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

/// Parses `<Contents>` items from an S3 `ListObjectsV2` XML response.
pub(crate) fn parse_s3_contents(xml: &str) -> Vec<S3ObjectItem> {
    let mut items = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<Contents>") {
        let after_start = &rest[start + 10..];
        let Some(end) = after_start.find("</Contents>") else {
            break;
        };
        let block = &after_start[..end];
        if let Some(key) = parse_xml_tag(block, "Key") {
            let size = parse_xml_tag(block, "Size")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            let last_modified = parse_xml_tag(block, "LastModified")
                .unwrap_or_default()
                .to_string();
            items.push(S3ObjectItem {
                key: key.to_string(),
                size,
                last_modified,
            });
        }
        rest = &after_start[end + 11..];
    }
    items
}

fn parse_xml_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = xml.find(&start_tag)? + start_tag.len();
    let end = xml[start..].find(&end_tag)? + start;
    Some(&xml[start..end])
}

#[cfg(test)]
/// The URI path of one backup object: the bucket path of the endpoint, the prefix and the file.
fn object_path(url: &url::Url, config: &S3Config, entry: &BackupEntry) -> String {
    let key = format!("{}/{}.dump.gz", entry.name, entry.id);
    object_key_path(url, config, &key)
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
            sha256: "deadbeef".to_string(),
            created_at: 1,
            verified_at: None,
            verification_status: None,
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
    fn prefix_already_present_in_key_is_not_duplicated() {
        let path = object_key_path(
            &url::Url::parse("https://s3.example.com/bucket").unwrap(),
            &config("https://s3.example.com/bucket", "databases"),
            "databases/shop/1a2b.dump.gz",
        );
        assert_eq!(path, "/bucket/databases/shop/1a2b.dump.gz");
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

    #[test]
    fn parses_s3_contents_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
    <Name>my-bucket</Name>
    <Contents>
        <Key>databases/shop/1a2b.dump.gz</Key>
        <LastModified>2026-10-21T18:00:00.000Z</LastModified>
        <ETag>&quot;d41d8cd98f00b204e9800998ecf8427e&quot;</ETag>
        <Size>1048576</Size>
    </Contents>
    <Contents>
        <Key>databases/shop/3c4d.dump.gz</Key>
        <LastModified>2026-10-21T19:00:00.000Z</LastModified>
        <Size>2097152</Size>
    </Contents>
</ListBucketResult>"#;
        let items = parse_s3_contents(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].key, "databases/shop/1a2b.dump.gz");
        assert_eq!(items[0].size, 1048576);
        assert_eq!(items[0].last_modified, "2026-10-21T18:00:00.000Z");
        assert_eq!(items[1].key, "databases/shop/3c4d.dump.gz");
        assert_eq!(items[1].size, 2097152);
    }

    #[test]
    fn query_string_is_canonicalized() {
        let conf = config("https://s3.example.com/bucket", "databases");
        let q = list_query_string(&conf, "shop");
        assert_eq!(q, "list-type=2&prefix=databases%2Fshop");

        let conf_empty = config("https://s3.example.com/bucket", "");
        let q_empty = list_query_string(&conf_empty, "");
        assert_eq!(q_empty, "list-type=2");
    }
}
