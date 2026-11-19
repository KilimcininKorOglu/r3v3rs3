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
    src: &std::path::Path,
) -> anyhow::Result<()> {
    let key = format!("{}/{}.dump.gz", entry.name, entry.id);
    upload_file(client, config, &key, src).await
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

/// The size of one part of a multipart upload. S3 needs at least 5 MiB for every part but the
/// last, and allows 10,000 parts; the panel holds one part in memory at a time.
pub(crate) const PART_SIZE: u64 = 16 * 1024 * 1024;

/// The scheme and host, the host header and the URI path of the object `key`.
struct ObjectUrl {
    base: String,
    host: String,
    path: String,
}

impl ObjectUrl {
    fn new(config: &S3Config, key: &str) -> anyhow::Result<Self> {
        let url = url::Url::parse(&config.endpoint).context("the S3 endpoint is not a URL")?;
        let scheme = url.scheme();
        if scheme != "https" && scheme != "http" {
            bail!("the S3 endpoint must be an http or https URL");
        }
        let host = host(&url);
        Ok(Self {
            base: format!("{scheme}://{host}"),
            path: object_key_path(&url, config, key),
            host,
        })
    }

    /// A request to the object, signed with SigV4 over `payload`. `query` is the canonical
    /// query string: sorted and encoded, without the `?`.
    fn signed(
        &self,
        config: &S3Config,
        (method, query): (Method, &str),
        payload: Bytes,
    ) -> anyhow::Result<Request<Full<Bytes>>> {
        let amz_date = sigv4::amz_date(OffsetDateTime::now_utc());
        let authorization = sigv4::authorization(
            &config.access_key,
            &config.secret_key,
            &SigningInput {
                method: method.as_str(),
                host: &self.host,
                path: &self.path,
                query,
                amz_date: &amz_date,
                payload: &payload,
                region: &config.region,
                service: "s3",
            },
        )?;
        let uri = match query {
            "" => format!("{}{}", self.base, self.path),
            _ => format!("{}{}?{query}", self.base, self.path),
        };
        Ok(Request::builder()
            .method(method)
            .uri(uri)
            .header("host", &self.host)
            .header("x-amz-date", &amz_date)
            .header(
                "x-amz-content-sha256",
                hex::encode(Sha256::digest(&payload)),
            )
            .header("authorization", authorization)
            .body(Full::new(payload))?)
    }
}

/// Sends a signed request and fails on a status that is not a success.
async fn send(
    client: &HttpClient,
    request: Request<Full<Bytes>>,
) -> anyhow::Result<http::Response<hyper::body::Incoming>> {
    let response = tokio::time::timeout(UPLOAD_TIMEOUT, client.request(request)).await??;
    let status = response.status();
    if !status.is_success() {
        bail!("the S3 bucket answered {status}");
    }
    Ok(response)
}

/// The body of a small answer, such as the XML of a multipart step.
async fn small_body(response: http::Response<hyper::body::Incoming>) -> anyhow::Result<String> {
    let body = tokio::time::timeout(UPLOAD_TIMEOUT, BodyExt::collect(response.into_body()))
        .await?
        .map_err(|err| anyhow::anyhow!("{err}"))?
        .to_bytes();
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Uploads the local file `src` under `{prefix}/{key}`. A file larger than one part goes up
/// as a multipart upload, one part at a time, so a backup of any size passes without being
/// held in memory.
pub(crate) async fn upload_file(
    client: &HttpClient,
    config: &S3Config,
    key: &str,
    src: &std::path::Path,
) -> anyhow::Result<()> {
    let size = tokio::fs::metadata(src).await?.len();
    if size <= PART_SIZE {
        return upload_key(client, config, key, tokio::fs::read(src).await?).await;
    }
    let object = ObjectUrl::new(config, key)?;
    let upload_id = create_multipart(client, config, &object).await?;
    let sent = upload_parts(client, config, (&object, &upload_id), src).await;
    let done = match sent {
        Ok(etags) => complete_multipart(client, config, (&object, &upload_id), &etags).await,
        Err(err) => Err(err),
    };
    if let Err(err) = &done
        && let Err(abort) = abort_multipart(client, config, (&object, &upload_id)).await
    {
        // The parts stay in the bucket until a lifecycle rule removes them.
        tracing::warn!(
            key,
            err = format!("{err:#}"),
            abort = format!("{abort:#}"),
            "the multipart upload could not be aborted"
        );
    }
    done
}

/// The canonical query of the requests of one multipart upload.
fn upload_query(upload_id: &str) -> String {
    format!("uploadId={}", utf8_percent_encode(upload_id, AWS_QUERY_SET))
}

async fn create_multipart(
    client: &HttpClient,
    config: &S3Config,
    object: &ObjectUrl,
) -> anyhow::Result<String> {
    let request = object.signed(config, (Method::POST, "uploads="), Bytes::new())?;
    let xml = small_body(send(client, request).await?).await?;
    let upload_id = parse_xml_tag(&xml, "UploadId")
        .context("the S3 bucket answered the multipart upload without an upload id")?;
    Ok(xml_unescape(upload_id))
}

/// Sends the parts of `src` one after another and returns their ETags in order.
async fn upload_parts(
    client: &HttpClient,
    config: &S3Config,
    (object, upload_id): (&ObjectUrl, &str),
    src: &std::path::Path,
) -> anyhow::Result<Vec<String>> {
    use tokio::io::AsyncReadExt as _;
    let mut file = tokio::fs::File::open(src).await?;
    let mut etags = Vec::new();
    loop {
        let mut part = Vec::with_capacity(PART_SIZE as usize);
        (&mut file).take(PART_SIZE).read_to_end(&mut part).await?;
        if part.is_empty() {
            return Ok(etags);
        }
        let number = etags.len() + 1;
        let query = format!("partNumber={number}&{}", upload_query(upload_id));
        let request = object.signed(config, (Method::PUT, &query), Bytes::from(part))?;
        let response = send(client, request).await?;
        let etag = response
            .headers()
            .get(http::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .with_context(|| format!("the S3 bucket answered part {number} without an ETag"))?;
        etags.push(etag.to_string());
    }
}

/// Joins the parts into the object. S3 can answer the completion with 200 and an error in the
/// body, so the body is checked too.
async fn complete_multipart(
    client: &HttpClient,
    config: &S3Config,
    (object, upload_id): (&ObjectUrl, &str),
    etags: &[String],
) -> anyhow::Result<()> {
    let body = complete_body(etags);
    let query = upload_query(upload_id);
    let request = object.signed(config, (Method::POST, &query), Bytes::from(body))?;
    let xml = small_body(send(client, request).await?).await?;
    if xml.contains("<Error>") {
        bail!("the S3 bucket refused to complete the multipart upload: {xml}");
    }
    Ok(())
}

/// The XML body of `CompleteMultipartUpload`.
fn complete_body(etags: &[String]) -> String {
    let mut body = String::from("<CompleteMultipartUpload>");
    for (index, etag) in etags.iter().enumerate() {
        let etag = etag
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        body.push_str(&format!(
            "<Part><PartNumber>{}</PartNumber><ETag>{etag}</ETag></Part>",
            index + 1
        ));
    }
    body.push_str("</CompleteMultipartUpload>");
    body
}

async fn abort_multipart(
    client: &HttpClient,
    config: &S3Config,
    (object, upload_id): (&ObjectUrl, &str),
) -> anyhow::Result<()> {
    let query = upload_query(upload_id);
    let request = object.signed(config, (Method::DELETE, &query), Bytes::new())?;
    send(client, request).await.map(|_| ())
}

/// Downloads `{prefix}/{key}` into the local file `dest` piece by piece and returns its size.
/// The object is outside input: a download that passes the room of the disk fails, and every
/// piece must arrive within the request timeout.
pub(crate) async fn download_to_file(
    client: &HttpClient,
    config: &S3Config,
    key: &str,
    (dest, room): (&std::path::Path, &mut super::transfer::DiskRoom),
) -> anyhow::Result<u64> {
    let object = ObjectUrl::new(config, key)?;
    let request = object.signed(config, (Method::GET, ""), Bytes::new())?;
    let response = send(client, request).await?;
    check_length(&response, room)?;
    write_body(response.into_body(), dest, room).await
}

/// Refuses an object whose announced length does not fit the room of the disk, before any of
/// it is written. The room is counted again while the body arrives.
fn check_length(
    response: &http::Response<hyper::body::Incoming>,
    room: &super::transfer::DiskRoom,
) -> anyhow::Result<()> {
    let length = response
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse::<u64>().ok());
    match length {
        Some(length) => room.fits(length),
        None => Ok(()),
    }
}

/// Writes the body into `dest` piece by piece within the room of the disk, which also holds
/// for a body that announced no length.
async fn write_body(
    mut body: hyper::body::Incoming,
    dest: &std::path::Path,
    room: &mut super::transfer::DiskRoom,
) -> anyhow::Result<u64> {
    use tokio::io::AsyncWriteExt as _;
    let mut out = tokio::fs::File::create(dest).await?;
    let mut size = 0u64;
    while let Some(frame) = tokio::time::timeout(UPLOAD_TIMEOUT, body.frame()).await? {
        let Ok(data) = frame.map_err(|err| anyhow::anyhow!("{err}"))?.into_data() else {
            continue;
        };
        room.take(data.len() as u64)?;
        size += data.len() as u64;
        out.write_all(&data).await?;
    }
    out.sync_all().await?;
    Ok(size)
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
                key: xml_unescape(key),
                size,
                last_modified,
            });
        }
        rest = &after_start[end + 11..];
    }
    items
}

pub(crate) fn xml_unescape(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp_pos) = rest.find('&') {
        out.push_str(&rest[..amp_pos]);
        let after_amp = &rest[amp_pos + 1..];
        if let Some(semi_pos) = after_amp.find(';') {
            let entity = &after_amp[..semi_pos];
            if decode_xml_entity(entity, &mut out) {
                rest = &after_amp[semi_pos + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after_amp;
    }
    out.push_str(rest);
    out
}

fn decode_xml_entity(entity: &str, out: &mut String) -> bool {
    match entity {
        "amp" => out.push('&'),
        "lt" => out.push('<'),
        "gt" => out.push('>'),
        "quot" => out.push('"'),
        "apos" => out.push('\''),
        _ if entity.starts_with('#') => return decode_numeric_entity(&entity[1..], out),
        _ => return false,
    }
    true
}

fn decode_numeric_entity(num_str: &str, out: &mut String) -> bool {
    let code_point = if let Some(hex) = num_str
        .strip_prefix('x')
        .or_else(|| num_str.strip_prefix('X'))
    {
        u32::from_str_radix(hex, 16).ok()
    } else {
        num_str.parse::<u32>().ok()
    };
    if let Some(ch) = code_point.and_then(char::from_u32) {
        out.push(ch);
        true
    } else {
        false
    }
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
    use super::super::transfer::DiskRoom;
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
            drill: None,
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
    fn parses_s3_contents_with_xml_entities() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
    <Name>my-bucket</Name>
    <Contents>
        <Key>databases/a&amp;b/snapshot_&lt;1&gt;_&quot;test&quot;_&apos;full&apos;.dump.gz</Key>
        <LastModified>2026-10-21T18:00:00.000Z</LastModified>
        <Size>1024</Size>
    </Contents>
    <Contents>
        <Key>databases/hex/item_&#x26;_&#38;.dump.gz</Key>
        <LastModified>2026-10-21T18:00:00.000Z</LastModified>
        <Size>2048</Size>
    </Contents>
</ListBucketResult>"#;
        let items = parse_s3_contents(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].key,
            "databases/a&b/snapshot_<1>_\"test\"_'full'.dump.gz"
        );
        assert_eq!(items[1].key, "databases/hex/item_&_&.dump.gz");
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

    type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, String, Vec<u8>)>>>;

    /// A bucket that answers the steps of a multipart upload, records the method, the path
    /// with the query and the body of every request, and fails the part `failing`.
    async fn multipart_bucket(failing: Option<usize>) -> (mockito::ServerGuard, Seen) {
        let mut server = mockito::Server::new_async().await;
        let seen: Seen = Default::default();
        for method in ["POST", "PUT", "DELETE"] {
            let record = seen.clone();
            server
                .mock(method, mockito::Matcher::Any)
                .with_header("etag", "\"part\"")
                .with_body_from_request(move |req| {
                    let body = req.body().cloned().unwrap_or_default();
                    let path = req.path_and_query().to_string();
                    record.lock().unwrap().push((method.into(), path, body));
                    if method == "POST" && req.path_and_query().ends_with("uploads=") {
                        return b"<InitiateMultipartUploadResult><UploadId>id+1</UploadId></InitiateMultipartUploadResult>".to_vec();
                    }
                    Vec::new()
                })
                .create_async()
                .await;
        }
        if let Some(part) = failing {
            server
                .mock("PUT", mockito::Matcher::Any)
                .match_query(mockito::Matcher::Regex(format!("partNumber={part}&")))
                .with_status(500)
                .create_async()
                .await;
        }
        (server, seen)
    }

    /// A file of `size` random bytes in a directory of its own.
    async fn random_file(
        size: usize,
    ) -> (crate::platform::tests::TempDir, std::path::PathBuf, Vec<u8>) {
        let dir = crate::platform::tests::TempDir(
            std::env::temp_dir().join(format!("r3v3rs3-s3-{:08x}", rand::random::<u32>())),
        );
        tokio::fs::create_dir_all(&dir.0).await.unwrap();
        let content: Vec<u8> = (0..size).map(|_| rand::random::<u8>()).collect();
        let path = dir.0.join("backup.dump.gz");
        tokio::fs::write(&path, &content).await.unwrap();
        (dir, path, content)
    }

    /// The S3 multipart API (CreateMultipartUpload, UploadPart, CompleteMultipartUpload): a
    /// backup larger than one part goes up in parts no larger than the part size, so the panel
    /// never holds the whole backup, and the parts joined in their order are the file.
    #[tokio::test]
    async fn a_file_larger_than_a_part_goes_up_in_parts() {
        let (server, seen) = multipart_bucket(None).await;
        let size = 2 * PART_SIZE as usize + 5;
        let (_dir, path, content) = random_file(size).await;
        let client = crate::cdn::fetch::build_client().await.unwrap();
        let config = config(&format!("{}/bucket", server.url()), "backups");
        upload_file(&client, &config, "db/1.dump.gz", &path)
            .await
            .unwrap();
        let seen = seen.lock().unwrap();
        let parts: Vec<_> = seen.iter().filter(|(m, _, _)| m == "PUT").collect();
        assert_eq!(parts.len(), 3);
        for (number, (_, path, body)) in parts.iter().enumerate() {
            assert!(
                path.contains(&format!("partNumber={}&uploadId=id%2B1", number + 1)),
                "{path}"
            );
            assert!(body.len() as u64 <= PART_SIZE);
        }
        let joined: Vec<u8> = parts.iter().flat_map(|(_, _, body)| body.clone()).collect();
        assert!(joined == content, "the parts do not join into the file");
        let (_, last_path, last_body) = seen.last().unwrap();
        assert!(last_path.ends_with("uploadId=id%2B1"), "{last_path}");
        let complete = String::from_utf8_lossy(last_body);
        assert!(
            complete.contains("<PartNumber>3</PartNumber>"),
            "{complete}"
        );
    }

    /// A part that the bucket refuses fails the upload, and the upload is aborted, so the parts
    /// sent before it do not stay in the bucket.
    #[tokio::test]
    async fn a_refused_part_aborts_the_upload() {
        let (server, seen) = multipart_bucket(Some(2)).await;
        let (_dir, path, _) = random_file(2 * PART_SIZE as usize + 5).await;
        let client = crate::cdn::fetch::build_client().await.unwrap();
        let config = config(&format!("{}/bucket", server.url()), "");
        let err = upload_file(&client, &config, "db/1.dump.gz", &path)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("500"), "{err:#}");
        let seen = seen.lock().unwrap();
        assert!(
            seen.iter()
                .any(|(m, p, _)| m == "DELETE" && p.ends_with("uploadId=id%2B1")),
            "no abort in {:?}",
            seen.iter().map(|(m, p, _)| (m, p)).collect::<Vec<_>>()
        );
        assert!(
            !seen
                .iter()
                .any(|(m, p, _)| m == "POST" && p.contains("uploadId="))
        );
    }

    /// An object from the bucket is outside input: a download lands in a file byte for byte,
    /// and an object that would leave the disk less than its reserve fails, also when the
    /// bucket sends no length. The reserve here leaves room for 1 MiB of the 3 MiB object.
    #[tokio::test]
    async fn a_download_lands_in_a_file_and_stops_at_the_room_of_the_disk() {
        let mut server = mockito::Server::new_async().await;
        let content: Vec<u8> = (0..3 * 1024 * 1024).map(|_| rand::random::<u8>()).collect();
        let sent = content.clone();
        server
            .mock("GET", "/bucket/sized")
            .with_body(content.clone())
            .create_async()
            .await;
        server
            .mock("GET", "/bucket/chunked")
            .with_chunked_body(move |w| w.write_all(&sent))
            .create_async()
            .await;
        let (dir, _, _) = random_file(0).await;
        let client = crate::cdn::fetch::build_client().await.unwrap();
        let config = config(&format!("{}/bucket", server.url()), "");
        let dest = dir.0.join("out");
        let mut room = DiskRoom::new(&dir.0).unwrap();
        let size = download_to_file(&client, &config, "sized", (&dest, &mut room))
            .await
            .unwrap();
        assert_eq!(size, content.len() as u64);
        assert!(tokio::fs::read(&dest).await.unwrap() == content);
        for key in ["sized", "chunked"] {
            let free = crate::platform::prune::get_disk_space(&dir.0)
                .unwrap()
                .available_bytes;
            let mut room = DiskRoom::with_reserve(&dir.0, free - 1024 * 1024).unwrap();
            let err = download_to_file(&client, &config, key, (&dest, &mut room))
                .await
                .unwrap_err();
            assert!(
                format!("{err:#}").contains("would keep less than"),
                "{key}: {err:#}"
            );
        }
    }
}
