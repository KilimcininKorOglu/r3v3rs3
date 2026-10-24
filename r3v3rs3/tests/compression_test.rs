use async_compression::tokio::bufread::{BrotliDecoder, GzipDecoder, ZstdDecoder};
use r3v3rs3_api::{
    compression::{Compression, CompressionAlgorithm},
    proxy::HttpProxy,
};
use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, ETAG, VARY};
use tokio::io::AsyncReadExt;

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
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
        let get = |path: &'static str, accept: &'static str| {
            client
                .get(port.http_url(path))
                .header(ACCEPT_ENCODING, accept)
                .send()
        };

        for (accept, expected) in [
            ("gzip, br, zstd", "zstd"),
            ("*", "zstd"),
            ("gzip, br;q=0.9", "gzip"),
            ("br", "br"),
        ] {
            let resp = get("/page", accept).await?;
            assert_eq!(resp.status(), 200);
            assert_eq!(encoding(&resp), Some(expected), "{accept}");
            assert!(resp.headers().get(CONTENT_LENGTH).is_none());
            assert_eq!(resp.headers()[VARY], "Accept-Encoding");
            assert_eq!(resp.headers()[ETAG], "W/\"v1\"");
            assert_eq!(decode(expected, &resp.bytes().await?).await?, page);
        }

        let resp = get("/stream", "gzip").await?;
        assert_eq!(encoding(&resp), Some("gzip"));
        assert_eq!(decode("gzip", &resp.bytes().await?).await?, page);

        for accept in ["identity", "gzip;q=0"] {
            let resp = get("/page", accept).await?;
            assert_eq!(encoding(&resp), None, "{accept}");
            assert_eq!(resp.headers()[VARY], "Accept-Encoding");
            assert_eq!(resp.headers()[ETAG], "\"v1\"");
            assert_eq!(resp.text().await?, page);
        }

        for path in ["/small", "/image", "/no-transform"] {
            let resp = get(path, "gzip").await?;
            assert_eq!(encoding(&resp), None, "{path}");
            assert!(resp.headers().get(VARY).is_none(), "{path}");
        }

        let resp = get("/encoded", "zstd").await?;
        assert_eq!(encoding(&resp), Some("gzip"));
        assert_eq!(resp.text().await?, page);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn shared_compression_dictionary_serving_and_dcz_encoding() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut upstream = mockito::Server::new_async().await;
    let dict_raw = "{\"schema\":\"v1\",\"users\":[".to_string();
    let body_payload =
        format!("{dict_raw}{{\"id\":1,\"name\":\"Alice\"}},{{\"id\":2,\"name\":\"Bob\"}}]}}");

    let _mock_api = upstream
        .mock("GET", "/api/users")
        .with_header("content-type", "application/json")
        .with_header("etag", "\"v1\"")
        .with_body(&body_payload)
        .create_async()
        .await;

    let mut dict = r3v3rs3_api::compression_dictionary::CompressionDictionary {
        id: r3v3rs3_api::id::ShortId::new(),
        name: "Users Schema".to_string(),
        match_pattern: "/api/*".to_string(),
        path: "/.well-known/dict/users.dict".to_string(),
        raw_data: dict_raw.clone(),
        sha256_sf: String::new(),
    };
    dict.ensure_hash();
    let dict_hash = dict.sha256_sf.clone();

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream.url(), None)],
        upgrade_insecure: false,
        compression: Compression {
            algorithms: vec![CompressionAlgorithm::Zstd, CompressionAlgorithm::Gzip],
            dictionaries: vec![dict.clone()],
            min_size: 10,
            ..Default::default()
        },
        ..Default::default()
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("port1", &port)])
        .proxies(vec![http_proxy_entry("proxy1", "port1", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::builder().no_gzip().no_brotli().build()?;

        // 1. Verify dictionary serving
        let dict_resp = client
            .get(port.http_url("/.well-known/dict/users.dict"))
            .send()
            .await?;
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
        assert_eq!(served_bytes, dict_raw);

        // 2. Request API endpoint with Available-Dictionary header
        let api_resp = client
            .get(port.http_url("/api/users"))
            .header(ACCEPT_ENCODING, "dcz, zstd, gzip")
            .header("available-dictionary", &dict_hash)
            .send()
            .await?;

        assert_eq!(api_resp.status(), 200);
        assert_eq!(encoding(&api_resp), Some("dcz"));
        let api_headers = api_resp.headers();
        assert_eq!(
            api_headers.get("content-dictionary").unwrap().to_str()?,
            dict_hash
        );
        assert_eq!(api_headers[ETAG], "W/\"v1\"");
        let vary_items = api_headers
            .get_all(VARY)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect::<Vec<_>>();
        assert!(vary_items.contains(&"Available-Dictionary"));
        assert!(vary_items.contains(&"Accept-Encoding"));

        // 3. Decompress response using dictionary
        let compressed_bytes = api_resp.bytes().await?;
        let mut decompressor = zstd::bulk::Decompressor::with_dictionary(dict_raw.as_bytes())?;
        let decompressed = decompressor.decompress(&compressed_bytes, 10 * 1024)?;
        assert_eq!(String::from_utf8(decompressed)?, body_payload);

        Ok(())
    })
    .await
}
