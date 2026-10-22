//! Executes unary gRPC calls over HTTP/2 with framing and metadata handling.

use super::proto::{encode_framed_grpc_message, json_to_protobuf, protobuf_to_json};
use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use r3v3rs3_api::grpc::{GrpcInvokeRequest, GrpcInvokeResponse, grpc_status_name};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const GRPC_INVOKE_TIMEOUT: Duration = Duration::from_secs(15);

fn build_http_client(plaintext_h2: bool) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(GRPC_INVOKE_TIMEOUT)
        .danger_accept_invalid_certs(true);

    if plaintext_h2 {
        builder = builder.http2_prior_knowledge();
    }
    builder.build().context("failed to build gRPC HTTP client")
}

fn apply_request_headers(headers: &mut HeaderMap, metadata: &HashMap<String, String>) {
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        HeaderValue::from_static("application/grpc"),
    );
    headers.insert(reqwest::header::TE, HeaderValue::from_static("trailers"));

    for (k, v) in metadata {
        if let Ok(name) = HeaderName::from_bytes(k.as_bytes())
            && let Ok(val) = HeaderValue::from_str(v)
        {
            headers.insert(name, val);
        }
    }
}

fn parse_grpc_web_trailers_payload(payload: &[u8], trailers: &mut HashMap<String, String>) {
    if let Ok(text) = std::str::from_utf8(payload) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once(':') {
                trailers.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
}

fn parse_body_frames(raw_body: &[u8]) -> (Vec<u8>, HashMap<String, String>) {
    let mut data_payload = Vec::new();
    let mut trailers = HashMap::new();
    let mut offset = 0;

    while offset + 5 <= raw_body.len() {
        let flag = raw_body[offset];
        let len = u32::from_be_bytes([
            raw_body[offset + 1],
            raw_body[offset + 2],
            raw_body[offset + 3],
            raw_body[offset + 4],
        ]) as usize;
        offset += 5;

        if offset + len > raw_body.len() {
            break;
        }
        let frame_slice = &raw_body[offset..offset + len];
        offset += len;

        if flag == 0x80 {
            parse_grpc_web_trailers_payload(frame_slice, &mut trailers);
        } else if data_payload.is_empty() {
            data_payload.extend_from_slice(frame_slice);
        }
    }

    if data_payload.is_empty() && raw_body.len() > 5 {
        data_payload.extend_from_slice(&raw_body[5..]);
    }

    (data_payload, trailers)
}

fn extract_grpc_status(
    headers: &HeaderMap,
    trailers: &HashMap<String, String>,
) -> (u32, String, Option<String>) {
    let status_str = headers
        .get("grpc-status")
        .and_then(|v| v.to_str().ok())
        .or_else(|| trailers.get("grpc-status").map(String::as_str));

    let code = status_str.and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    let name = grpc_status_name(code).to_string();

    let message = headers
        .get("grpc-message")
        .and_then(|v| v.to_str().ok())
        .or_else(|| trailers.get("grpc-message").map(String::as_str))
        .map(|msg| {
            percent_encoding::percent_decode_str(msg)
                .decode_utf8_lossy()
                .into_owned()
        });

    (code, name, message)
}

fn format_endpoint_url(base: &str, service: &str, method: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    format!("{trimmed}/{service}/{method}")
}

/// Invokes a gRPC unary method on the target endpoint.
pub async fn invoke_grpc_unary(
    target_url: &str,
    req: &GrpcInvokeRequest,
    schema: Option<&HashMap<String, u32>>,
) -> Result<GrpcInvokeResponse> {
    let is_plaintext = target_url.starts_with("http://");
    let client = build_http_client(is_plaintext)?;
    let endpoint = format_endpoint_url(target_url, &req.service, &req.method);

    let proto_bytes = json_to_protobuf(&req.payload, schema);
    let framed_request = encode_framed_grpc_message(&proto_bytes);

    let mut headers = HeaderMap::new();
    apply_request_headers(&mut headers, &req.metadata);

    let start = Instant::now();
    let response = client
        .post(&endpoint)
        .headers(headers)
        .body(framed_request.to_vec())
        .send()
        .await
        .context("failed to send gRPC request to upstream")?;

    let latency_ms = start.elapsed().as_millis() as u64;

    let res_headers_raw = response.headers().clone();
    let res_bytes = response
        .bytes()
        .await
        .context("failed to read gRPC response bytes")?;

    let (data_payload, body_trailers) = parse_body_frames(&res_bytes);
    let (code, name, msg) = extract_grpc_status(&res_headers_raw, &body_trailers);

    let json_payload = if data_payload.is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        protobuf_to_json(&data_payload, None)
    };

    let mut headers_map = HashMap::new();
    for (k, v) in res_headers_raw.iter() {
        if let Ok(val) = v.to_str() {
            headers_map.insert(k.as_str().to_string(), val.to_string());
        }
    }

    Ok(GrpcInvokeResponse {
        grpc_status: code,
        grpc_status_name: name,
        grpc_message: msg,
        latency_ms,
        headers: headers_map,
        trailers: body_trailers,
        payload: json_payload,
        raw_bytes_base64: Some(STANDARD.encode(&data_payload)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_endpoint_url() {
        assert_eq!(
            format_endpoint_url("http://localhost:50051", "helloworld.Greeter", "SayHello"),
            "http://localhost:50051/helloworld.Greeter/SayHello"
        );
        assert_eq!(
            format_endpoint_url("http://localhost:50051/", "helloworld.Greeter", "SayHello"),
            "http://localhost:50051/helloworld.Greeter/SayHello"
        );
    }

    #[test]
    fn test_parse_body_frames_with_grpc_web_trailers() {
        let mut framed = Vec::new();
        // Frame 1: data (flag 0)
        framed.push(0x00);
        framed.extend_from_slice(&6u32.to_be_bytes());
        framed.extend_from_slice(b"sample");

        // Frame 2: trailers (flag 0x80)
        let trailer_str = "grpc-status: 0\r\ngrpc-message: Success\r\n";
        framed.push(0x80);
        framed.extend_from_slice(&(trailer_str.len() as u32).to_be_bytes());
        framed.extend_from_slice(trailer_str.as_bytes());

        let (payload, trailers) = parse_body_frames(&framed);
        assert_eq!(payload, b"sample");
        assert_eq!(trailers.get("grpc-status").unwrap(), "0");
        assert_eq!(trailers.get("grpc-message").unwrap(), "Success");
    }
}
