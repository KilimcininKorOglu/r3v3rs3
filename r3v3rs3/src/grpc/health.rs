//! gRPC Health Checking Protocol implementation (grpc.health.v1.Health).

use super::proto;
use bytes::Bytes;
use r3v3rs3_api::grpc::{GrpcHealthCheckResponse, GrpcServingStatus, grpc_status_name};
use reqwest::header::{HeaderMap, HeaderValue};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const HEALTH_CHECK_PATH: &str = "/grpc.health.v1.Health/Check";

/// Encodes a `HealthCheckRequest` message into a framed gRPC message.
///
/// Protobuf schema:
/// ```proto
/// message HealthCheckRequest {
///   string service = 1;
/// }
/// ```
pub fn encode_health_check_request(service: &str) -> Bytes {
    let mut payload = Vec::new();
    if !service.is_empty() {
        proto::write_string_field(&mut payload, 1, service);
    }
    proto::encode_framed_grpc_message(&payload)
}

/// Parses a `HealthCheckResponse` protobuf payload.
///
/// Protobuf schema:
/// ```proto
/// message HealthCheckResponse {
///   enum ServingStatus {
///     UNKNOWN = 0;
///     SERVING = 1;
///     NOT_SERVING = 2;
///     SERVICE_UNKNOWN = 3;
///   }
///   ServingStatus status = 1;
/// }
/// ```
pub fn parse_health_check_response(payload: &[u8]) -> GrpcServingStatus {
    let mut offset = 0;
    while offset < payload.len() {
        let Some(tag_raw) = proto::read_varint(payload, &mut offset) else {
            break;
        };
        let field_num = (tag_raw >> 3) as u32;
        let wire_type = (tag_raw & 0x07) as u8;
        if field_num == 1 && wire_type == 0 {
            if let Some(val) = proto::read_varint(payload, &mut offset) {
                return GrpcServingStatus::from_u64(val);
            }
            break;
        } else {
            skip_wire_field(payload, &mut offset, wire_type);
        }
    }
    GrpcServingStatus::Unknown
}

fn skip_wire_field(payload: &[u8], offset: &mut usize, wire_type: u8) {
    match wire_type {
        0 => {
            let _ = proto::read_varint(payload, offset);
        }
        1 => {
            *offset = (*offset + 8).min(payload.len());
        }
        2 => {
            if let Some(len) = proto::read_varint(payload, offset) {
                *offset = (*offset + len as usize).min(payload.len());
            }
        }
        5 => {
            *offset = (*offset + 4).min(payload.len());
        }
        _ => {}
    }
}

pub fn build_grpc_health_client(
    is_plaintext: bool,
    timeout: Duration,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(true);

    if is_plaintext {
        builder = builder.http2_prior_knowledge();
    }
    builder.build().map_err(|e| e.to_string())
}

fn format_health_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    format!("{trimmed}{HEALTH_CHECK_PATH}")
}

fn create_health_request_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        HeaderValue::from_static("application/grpc"),
    );
    headers.insert(reqwest::header::TE, HeaderValue::from_static("trailers"));
    headers
}

fn parse_response_trailers(
    headers: &HeaderMap,
    body_trailers: &HashMap<String, String>,
) -> (u32, String, Option<String>) {
    let status_str = headers
        .get("grpc-status")
        .and_then(|v| v.to_str().ok())
        .or_else(|| body_trailers.get("grpc-status").map(String::as_str));

    let code = status_str.and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    let name = grpc_status_name(code).to_string();

    let message = headers
        .get("grpc-message")
        .and_then(|v| v.to_str().ok())
        .or_else(|| body_trailers.get("grpc-message").map(String::as_str))
        .map(|msg| {
            percent_encoding::percent_decode_str(msg)
                .decode_utf8_lossy()
                .into_owned()
        });

    (code, name, message)
}

/// Executes a unary `grpc.health.v1.Health/Check` RPC against the upstream target.
pub async fn check_grpc_health(
    client: &reqwest::Client,
    target_url: &str,
    service: &str,
) -> Result<GrpcHealthCheckResponse, String> {
    let url = format_health_url(target_url);
    let framed_request = encode_health_check_request(service);
    let headers = create_health_request_headers();

    let start = Instant::now();
    let response = client
        .post(&url)
        .headers(headers)
        .body(framed_request.to_vec())
        .send()
        .await
        .map_err(|e| format!("connection to {url} failed: {e}"))?;

    let latency_ms = start.elapsed().as_millis() as u64;

    if !response.status().is_success() {
        return Err(format!(
            "upstream returned HTTP status {}",
            response.status()
        ));
    }

    let headers_raw = response.headers().clone();
    let body_bytes = response
        .bytes()
        .await
        .map_err(|e| format!("failed reading response: {e}"))?;

    let (payload, body_trailers) = parse_grpc_body_frames(&body_bytes);
    let (code, name, msg) = parse_response_trailers(&headers_raw, &body_trailers);

    if code != 0 {
        return Ok(GrpcHealthCheckResponse {
            healthy: false,
            status: GrpcServingStatus::Unknown,
            status_code: 0,
            grpc_status: code,
            grpc_status_name: name,
            grpc_message: msg,
            latency_ms,
        });
    }

    let serving_status = parse_health_check_response(&payload);
    let healthy = serving_status.is_serving();

    Ok(GrpcHealthCheckResponse {
        healthy,
        status: serving_status,
        status_code: serving_status as u32,
        grpc_status: 0,
        grpc_status_name: "OK".to_string(),
        grpc_message: msg,
        latency_ms,
    })
}

fn parse_grpc_body_frames(raw_body: &[u8]) -> (Vec<u8>, HashMap<String, String>) {
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
            parse_trailer_lines(frame_slice, &mut trailers);
        } else if data_payload.is_empty() {
            data_payload.extend_from_slice(frame_slice);
        }
    }

    if data_payload.is_empty() && raw_body.len() > 5 {
        data_payload.extend_from_slice(&raw_body[5..]);
    }

    (data_payload, trailers)
}

fn parse_trailer_lines(payload: &[u8], trailers: &mut HashMap<String, String>) {
    if let Ok(text) = std::str::from_utf8(payload) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once(':') {
                trailers.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_empty_service_health_check() {
        let framed = encode_health_check_request("");
        assert_eq!(framed.len(), 5);
        assert_eq!(&framed[..], &[0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_encode_with_service_health_check() {
        let service = "test.Service";
        let framed = encode_health_check_request(service);
        assert_eq!(framed[0], 0x00);
        let len = u32::from_be_bytes([framed[1], framed[2], framed[3], framed[4]]) as usize;
        assert_eq!(len, framed.len() - 5);
        // Field tag 1 (string): 0x0a
        assert_eq!(framed[5], 0x0a);
        assert_eq!(framed[6], service.len() as u8);
        assert_eq!(&framed[7..], service.as_bytes());
    }

    #[test]
    fn test_parse_health_check_response_statuses() {
        // SERVING = 1: Tag (1 << 3 | 0) = 0x08, value 0x01
        let payload_serving = vec![0x08, 0x01];
        assert_eq!(
            parse_health_check_response(&payload_serving),
            GrpcServingStatus::Serving
        );

        // NOT_SERVING = 2: Tag 0x08, value 0x02
        let payload_not_serving = vec![0x08, 0x02];
        assert_eq!(
            parse_health_check_response(&payload_not_serving),
            GrpcServingStatus::NotServing
        );

        // SERVICE_UNKNOWN = 3: Tag 0x08, value 0x03
        let payload_unknown_service = vec![0x08, 0x03];
        assert_eq!(
            parse_health_check_response(&payload_unknown_service),
            GrpcServingStatus::ServiceUnknown
        );

        // UNKNOWN = 0
        let payload_unknown = vec![0x08, 0x00];
        assert_eq!(
            parse_health_check_response(&payload_unknown),
            GrpcServingStatus::Unknown
        );

        // Empty payload defaults to UNKNOWN
        assert_eq!(parse_health_check_response(&[]), GrpcServingStatus::Unknown);
    }
}
