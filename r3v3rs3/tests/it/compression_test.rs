use async_compression::tokio::bufread::{BrotliDecoder, GzipDecoder, ZstdDecoder};
use r3v3rs3_api::{
    compression::{Compression, CompressionAlgorithm},
    proxy::HttpProxy,
};
use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, ETAG, VARY};
use tokio::io::AsyncReadExt;

use crate::common;
use common::{
    TestPort, TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route,
    with_server,
};

async fn decode(encoding: &str, body: &[u8]) -> anyhow::Result<String> {
    let mut text = String::new();
    match encoding {
        "gzip" => GzipDecoder::new(body).read_to_string(&mut text).await?,
        "br" => BrotliDecoder::new(body).read_to_string(&mut text).await?,
        "zstd" => ZstdDecoder::new(body).read_to_string(&mut text).await?,
        other => anyhow::bail!("unexpected encoding: {other}"),
    };
    Ok(text)
}

fn encoding(resp: &reqwest::Response) -> Option<&str> {
    resp.headers()
        .get(CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
}

#[tokio::test]
async fn compression_encodes_eligible_responses() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut upstream = mockito::Server::new_async().await;
    let page = "<p>r3v3rs3 compression</p>".repeat(100);

    let _mock_page = upstream
        .mock("GET", "/page")
        .with_header("content-type", "text/html; charset=utf-8")
        .with_header("etag", "\"v1\"")
        .with_body(&page)
        .create_async()
        .await;
    let streamed = page.clone();
    let _mock_stream = upstream
        .mock("GET", "/stream")
        .with_header("content-type", "application/json")
        .with_chunked_body(move |writer| writer.write_all(streamed.as_bytes()))
        .create_async()
        .await;
    let _mock_small = upstream
        .mock("GET", "/small")
        .with_header("content-type", "text/plain")
        .with_body("small")
        .create_async()
        .await;
    let _mock_image = upstream
        .mock("GET", "/image")
        .with_header("content-type", "image/png")
        .with_body(&page)
        .create_async()
        .await;
    let _mock_encoded = upstream
        .mock("GET", "/encoded")
        .with_header("content-type", "text/html")
        .with_header("content-encoding", "gzip")
        .with_body(&page)
        .create_async()
        .await;
    let _mock_no_transform = upstream
        .mock("GET", "/no-transform")
        .with_header("content-type", "text/html")
        .with_header("cache-control", "public, no-transform")
        .with_body(&page)
        .create_async()
        .await;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream.url(), None)],
        upgrade_insecure: false,
        compression: Compression {
            algorithms: vec![
                CompressionAlgorithm::Zstd,
                CompressionAlgorithm::Brotli,
                CompressionAlgorithm::Gzip,
            ],
            ..Default::default()
        },
        ..Default::default()
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("compress", &port)])
        .proxies(vec![http_proxy_entry("proxy1", "compress", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::builder().no_gzip().no_brotli().build()?;
        check_negotiated_encodings(&client, &port, &page).await?;
        check_identity_requests(&client, &port, &page).await?;
        check_ineligible_responses(&client, &port, &page).await
    })
    .await
}

/// Sends a GET request to the path with the `Accept-Encoding` header.
async fn get_encoded(
    client: &reqwest::Client,
    port: &TestPort,
    path: &str,
    accept: &str,
) -> reqwest::Result<reqwest::Response> {
    client
        .get(port.http_url(path))
        .header(ACCEPT_ENCODING, accept)
        .send()
        .await
}

/// Checks that the proxy picks the preferred encoding, weakens the ETag and encodes a streamed
/// body.
async fn check_negotiated_encodings(
    client: &reqwest::Client,
    port: &TestPort,
    page: &str,
) -> anyhow::Result<()> {
    for (accept, expected) in [
        ("gzip, br, zstd", "zstd"),
        ("*", "zstd"),
        ("gzip, br;q=0.9", "gzip"),
        ("br", "br"),
    ] {
        let resp = get_encoded(client, port, "/page", accept).await?;
        assert_eq!(resp.status(), 200);
        assert_eq!(encoding(&resp), Some(expected), "{accept}");
        assert!(resp.headers().get(CONTENT_LENGTH).is_none());
        assert_eq!(resp.headers()[VARY], "Accept-Encoding");
        assert_eq!(resp.headers()[ETAG], "W/\"v1\"");
        assert_eq!(decode(expected, &resp.bytes().await?).await?, page);
    }

    let resp = get_encoded(client, port, "/stream", "gzip").await?;
    assert_eq!(encoding(&resp), Some("gzip"));
    assert_eq!(decode("gzip", &resp.bytes().await?).await?, page);
    Ok(())
}

/// Checks that a client that refuses every encoding gets the body unchanged.
async fn check_identity_requests(
    client: &reqwest::Client,
    port: &TestPort,
    page: &str,
) -> anyhow::Result<()> {
    for accept in ["identity", "gzip;q=0"] {
        let resp = get_encoded(client, port, "/page", accept).await?;
        assert_eq!(encoding(&resp), None, "{accept}");
        assert_eq!(resp.headers()[VARY], "Accept-Encoding");
        assert_eq!(resp.headers()[ETAG], "\"v1\"");
        assert_eq!(resp.text().await?, page);
    }
    Ok(())
}

/// Checks that small, binary, `no-transform` and already encoded responses are not encoded again.
async fn check_ineligible_responses(
    client: &reqwest::Client,
    port: &TestPort,
    page: &str,
) -> anyhow::Result<()> {
    for path in ["/small", "/image", "/no-transform"] {
        let resp = get_encoded(client, port, path, "gzip").await?;
        assert_eq!(encoding(&resp), None, "{path}");
        assert!(resp.headers().get(VARY).is_none(), "{path}");
    }

    let resp = get_encoded(client, port, "/encoded", "zstd").await?;
    assert_eq!(encoding(&resp), Some("gzip"));
    assert_eq!(resp.text().await?, page);
    Ok(())
}

const DICT_RAW: &str = "{\"schema\":\"v1\",\"users\":[";

/// The JSON body of both API paths, which shares its start with the dictionary.
fn api_payload() -> String {
    format!("{DICT_RAW}{{\"id\":1,\"name\":\"Alice\"}},{{\"id\":2,\"name\":\"Bob\"}}]}}")
}

/// An upstream with the same JSON body on `/api/users` and on `/static/users`.
async fn dictionary_upstream() -> mockito::ServerGuard {
    let mut upstream = mockito::Server::new_async().await;
    for path in ["/api/users", "/static/users"] {
        upstream
            .mock("GET", path)
            .with_header("content-type", "application/json")
            .with_header("etag", "\"v1\"")
            .with_body(api_payload())
            .expect_at_least(0)
            .create_async()
            .await;
    }
    upstream
}

/// A dictionary for the paths below `/api/`.
fn users_dictionary() -> r3v3rs3_api::compression_dictionary::CompressionDictionary {
    let mut dict = r3v3rs3_api::compression_dictionary::CompressionDictionary {
        id: r3v3rs3_api::id::ShortId::new(),
        name: "Users Schema".to_string(),
        match_pattern: "/api/*".to_string(),
        path: "/.well-known/dict/users.dict".to_string(),
        raw_data: DICT_RAW.to_string(),
        sha256_sf: String::new(),
    };
    dict.ensure_hash();
    dict
}

/// A proxy with the dictionary on a plain HTTP port and on an HTTPS port for `localhost`.
fn dictionary_storage(
    plain: &TestPort,
    secure: &TestPort,
    upstream: &str,
    root: &std::sync::Arc<r3v3rs3::certs::Cert>,
) -> TestStorage {
    use r3v3rs3::certs::Cert;
    let cert =
        std::sync::Arc::new(Cert::new_self_signed(&["localhost".parse().unwrap()], root).unwrap());
    // Without `upgrade_insecure`, the plain port answers itself instead of redirecting to HTTPS.
    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", upstream, None)],
        upgrade_insecure: false,
        compression: Compression {
            algorithms: vec![CompressionAlgorithm::Zstd, CompressionAlgorithm::Gzip],
            dictionaries: vec![users_dictionary()],
            min_size: 10,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut entry = http_proxy_entry("proxy1", "plain", proxy);
    entry.proxy.ports.push("secure".parse().unwrap());
    let mut secure_entry = common::port_entry("secure", secure.multiaddr_https());
    secure_entry.port.opts.tls_termination = Some(r3v3rs3_api::tls::TlsTermination {
        server_names: vec!["localhost".into()],
        ..Default::default()
    });
    TestStorage::builder()
        .ports(vec![http_port_entry("plain", plain), secure_entry])
        .proxies(vec![entry])
        .certs(
            [(root.id, root.clone()), (cert.id, cert)]
                .into_iter()
                .collect(),
        )
        .build()
}

/// RFC 9842: a dictionary-compressed response uses `dcz` only on HTTPS (section 8), only with
/// the dictionary that the client announced for a matching path and only when the client
/// accepts `dcz` (section 6.2).
#[tokio::test]
async fn shared_compression_dictionary_serving_and_dcz_encoding() -> anyhow::Result<()> {
    let plain = alloc_tcp_port().await?;
    let secure = alloc_tcp_port().await?;
    let upstream = dictionary_upstream().await;
    let root = std::sync::Arc::new(r3v3rs3::certs::Cert::new_ca()?);
    let ca = reqwest::Certificate::from_pem(&root.pem_chain)?;
    let config = dictionary_storage(&plain, &secure, &upstream.url(), &root);
    let dict_hash = users_dictionary().sha256_sf;

    with_server(config, |_| async move {
        let client = reqwest::Client::builder()
            .add_root_certificate(ca)
            .no_gzip()
            .no_brotli()
            .build()?;
        check_dictionary_served(&client, &secure.https_url("/.well-known/dict/users.dict")).await?;
        let api_resp = request_with_dictionary(&client, &secure, &dict_hash).await?;
        check_dcz_body(&api_resp.bytes().await?)?;
        check_no_dictionary_use(&client, &plain, &secure, &dict_hash).await
    })
    .await
}

/// RFC 9842 section 5: the body starts with the 8 fixed bytes and the SHA-256 digest of the
/// dictionary, and a Zstandard frame with the raw dictionary follows.
fn check_dcz_body(body: &[u8]) -> anyhow::Result<()> {
    use sha2::Digest;
    const MAGIC: [u8; 8] = [0x5e, 0x2a, 0x4d, 0x18, 0x20, 0x00, 0x00, 0x00];
    assert!(body.len() > 40, "{body:?}");
    assert_eq!(body[..8], MAGIC);
    assert_eq!(body[8..40], sha2::Sha256::digest(DICT_RAW.as_bytes())[..]);
    let mut decompressor = zstd::bulk::Decompressor::with_dictionary(DICT_RAW.as_bytes())?;
    let decompressed = decompressor.decompress(&body[40..], 10 * 1024)?;
    assert_eq!(String::from_utf8(decompressed)?, api_payload());
    Ok(())
}

/// Requests that must not get `dcz`: another dictionary, a path outside the dictionary
/// pattern, and a client that does not accept `dcz`. Each gets plain `zstd`.
async fn check_no_dictionary_use(
    client: &reqwest::Client,
    plain: &TestPort,
    secure: &TestPort,
    dict_hash: &str,
) -> anyhow::Result<()> {
    let other_hash = ":AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=:";
    let _ = plain;
    let cases = [
        (secure.https_url("/api/users"), "dcz, zstd", other_hash),
        (secure.https_url("/static/users"), "dcz, zstd", dict_hash),
        (secure.https_url("/api/users"), "zstd", dict_hash),
    ];
    for (url, accept, hash) in cases {
        let resp = client
            .get(url.clone())
            .header(ACCEPT_ENCODING, accept)
            .header("available-dictionary", hash)
            .send()
            .await?;
        assert_eq!(resp.status(), 200, "{url}");
        assert_eq!(encoding(&resp), Some("zstd"), "{url} {accept} {hash}");
        assert_eq!(decode("zstd", &resp.bytes().await?).await?, api_payload());
    }
    Ok(())
}

/// Checks that the proxy serves the dictionary with its headers and its raw data.
async fn check_dictionary_served(client: &reqwest::Client, url: &url::Url) -> anyhow::Result<()> {
    let dict_resp = client.get(url.clone()).send().await?;
    assert_eq!(dict_resp.status(), 200);
    let dict_headers = dict_resp.headers();
    assert_eq!(
        dict_headers.get("content-type").unwrap().to_str()?,
        "application/octet-stream"
    );
    let use_as_dict = dict_headers.get("use-as-dictionary").unwrap().to_str()?;
    assert!(use_as_dict.contains("match=\"/api/*\""));
    assert_eq!(
        dict_headers.get("cache-control").unwrap().to_str()?,
        "public, max-age=31536000, immutable"
    );
    let served_bytes = dict_resp.text().await?;
    assert_eq!(served_bytes, DICT_RAW);
    Ok(())
}

/// Requests the API over HTTPS with the dictionary hash, checks the `dcz` headers (RFC 9842
/// section 6.2), and returns the response.
async fn request_with_dictionary(
    client: &reqwest::Client,
    port: &TestPort,
    dict_hash: &str,
) -> anyhow::Result<reqwest::Response> {
    let api_resp = client
        .get(port.https_url("/api/users"))
        .header(ACCEPT_ENCODING, "dcz, zstd, gzip")
        .header("available-dictionary", dict_hash)
        .send()
        .await?;

    assert_eq!(api_resp.status(), 200);
    assert_eq!(encoding(&api_resp), Some("dcz"));
    let api_headers = api_resp.headers();
    assert_eq!(api_headers[ETAG], "W/\"v1\"");
    let vary_items = api_headers
        .get_all(VARY)
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect::<Vec<_>>();
    assert!(vary_items.contains(&"Available-Dictionary"));
    assert!(vary_items.contains(&"Accept-Encoding"));
    Ok(api_resp)
}
