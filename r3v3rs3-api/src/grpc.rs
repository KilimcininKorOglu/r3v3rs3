//! gRPC Server Reflection and API call tester types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// Standard gRPC status code names per the gRPC specification.
pub fn grpc_status_name(code: u32) -> &'static str {
    match code {
        0 => "OK",
        1 => "CANCELLED",
        2 => "UNKNOWN",
        3 => "INVALID_ARGUMENT",
        4 => "DEADLINE_EXCEEDED",
        5 => "NOT_FOUND",
        6 => "ALREADY_EXISTS",
        7 => "PERMISSION_DENIED",
        8 => "RESOURCE_EXHAUSTED",
        9 => "FAILED_PRECONDITION",
        10 => "ABORTED",
        11 => "OUT_OF_RANGE",
        12 => "UNIMPLEMENTED",
        13 => "INTERNAL",
        14 => "UNAVAILABLE",
        15 => "DATA_LOSS",
        16 => "UNAUTHENTICATED",
        _ => "UNKNOWN_STATUS",
    }
}

/// A discovered gRPC service with its available methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GrpcServiceInfo {
    pub name: String,
    pub methods: Vec<GrpcMethodInfo>,
}

/// A gRPC method schema and metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GrpcMethodInfo {
    pub name: String,
    pub full_name: String,
    pub input_type: String,
    pub output_type: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_request_json: Option<String>,
}

/// Request to discover gRPC services and methods using Server Reflection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GrpcReflectRequest {
    /// Upstream target URL (e.g. `http://127.0.0.1:50051`).
    /// If omitted on a proxy, the proxy's first upstream server is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_url: Option<String>,
    /// Optional service name to filter or query detailed schemas for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
}

/// Response containing discovered gRPC services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GrpcReflectResponse {
    pub services: Vec<GrpcServiceInfo>,
    pub reflection_version: String,
}

/// Request to invoke a unary gRPC method with a JSON payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GrpcInvokeRequest {
    /// Upstream target URL (optional, defaults to proxy upstream).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_url: Option<String>,
    /// Full service name (e.g. `helloworld.Greeter`).
    pub service: String,
    /// Method name (e.g. `SayHello`).
    pub method: String,
    /// JSON payload to send as the message body.
    pub payload: serde_json::Value,
    /// Optional metadata headers (e.g. `authorization: Bearer ...`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// Response of an invoked gRPC call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GrpcInvokeResponse {
    pub grpc_status: u32,
    pub grpc_status_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc_message: Option<String>,
    pub latency_ms: u64,
    pub headers: HashMap<String, String>,
    pub trailers: HashMap<String, String>,
    pub payload: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_bytes_base64: Option<String>,
}

/// Serving status per the grpc.health.v1.Health protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GrpcServingStatus {
    Unknown = 0,
    Serving = 1,
    NotServing = 2,
    ServiceUnknown = 3,
}

impl GrpcServingStatus {
    pub fn from_u64(val: u64) -> Self {
        match val {
            1 => Self::Serving,
            2 => Self::NotServing,
            3 => Self::ServiceUnknown,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Serving => "SERVING",
            Self::NotServing => "NOT_SERVING",
            Self::ServiceUnknown => "SERVICE_UNKNOWN",
            Self::Unknown => "UNKNOWN",
        }
    }

    pub fn is_serving(&self) -> bool {
        matches!(self, Self::Serving)
    }
}

/// Request to check gRPC upstream health status (grpc.health.v1.Health/Check).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GrpcHealthCheckRequest {
    /// Upstream target URL (e.g. `http://127.0.0.1:50051`).
    /// If omitted on a proxy, the proxy's first upstream server is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_url: Option<String>,
    /// Optional service name to check. Empty string checks overall server health.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
}

/// Response of a gRPC health check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GrpcHealthCheckResponse {
    pub healthy: bool,
    pub status: GrpcServingStatus,
    pub status_code: u32,
    pub grpc_status: u32,
    pub grpc_status_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc_message: Option<String>,
    pub latency_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grpc_status_names() {
        assert_eq!(grpc_status_name(0), "OK");
        assert_eq!(grpc_status_name(1), "CANCELLED");
        assert_eq!(grpc_status_name(3), "INVALID_ARGUMENT");
        assert_eq!(grpc_status_name(5), "NOT_FOUND");
        assert_eq!(grpc_status_name(14), "UNAVAILABLE");
        assert_eq!(grpc_status_name(16), "UNAUTHENTICATED");
        assert_eq!(grpc_status_name(99), "UNKNOWN_STATUS");
    }

    #[test]
    fn test_grpc_reflect_response_serde() {
        let resp = GrpcReflectResponse {
            services: vec![GrpcServiceInfo {
                name: "helloworld.Greeter".to_string(),
                methods: vec![GrpcMethodInfo {
                    name: "SayHello".to_string(),
                    full_name: "/helloworld.Greeter/SayHello".to_string(),
                    input_type: "helloworld.HelloRequest".to_string(),
                    output_type: "helloworld.HelloReply".to_string(),
                    client_streaming: false,
                    server_streaming: false,
                    sample_request_json: Some(r#"{"name": "Alice"}"#.to_string()),
                }],
            }],
            reflection_version: "v1".to_string(),
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GrpcReflectResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_grpc_invoke_response_serde() {
        let resp = GrpcInvokeResponse {
            grpc_status: 0,
            grpc_status_name: "OK".to_string(),
            grpc_message: None,
            latency_ms: 12,
            headers: HashMap::new(),
            trailers: HashMap::from([("grpc-status".to_string(), "0".to_string())]),
            payload: serde_json::json!({"message": "Hello Alice"}),
            raw_bytes_base64: None,
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GrpcInvokeResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_grpc_serving_status() {
        assert_eq!(GrpcServingStatus::from_u64(1), GrpcServingStatus::Serving);
        assert_eq!(
            GrpcServingStatus::from_u64(2),
            GrpcServingStatus::NotServing
        );
        assert_eq!(
            GrpcServingStatus::from_u64(3),
            GrpcServingStatus::ServiceUnknown
        );
        assert_eq!(GrpcServingStatus::from_u64(0), GrpcServingStatus::Unknown);
        assert_eq!(GrpcServingStatus::from_u64(99), GrpcServingStatus::Unknown);

        assert!(GrpcServingStatus::Serving.is_serving());
        assert!(!GrpcServingStatus::NotServing.is_serving());
        assert!(!GrpcServingStatus::ServiceUnknown.is_serving());
        assert!(!GrpcServingStatus::Unknown.is_serving());

        assert_eq!(GrpcServingStatus::Serving.as_str(), "SERVING");
        assert_eq!(GrpcServingStatus::NotServing.as_str(), "NOT_SERVING");
    }

    #[test]
    fn test_grpc_health_check_response_serde() {
        let resp = GrpcHealthCheckResponse {
            healthy: true,
            status: GrpcServingStatus::Serving,
            status_code: 1,
            grpc_status: 0,
            grpc_status_name: "OK".to_string(),
            grpc_message: None,
            latency_ms: 5,
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GrpcHealthCheckResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }
}
