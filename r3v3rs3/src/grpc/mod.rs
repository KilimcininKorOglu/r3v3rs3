//! gRPC Server Reflection and interactive call tester engine.

pub mod health;
pub mod invoke;
pub mod proto;
pub mod reflection;

use anyhow::{Context, Result};
use r3v3rs3_api::grpc::{
    GrpcInvokeRequest, GrpcInvokeResponse, GrpcMethodInfo, GrpcReflectResponse, GrpcServiceInfo,
};
use std::collections::HashMap;
use std::time::Duration;

const REFLECTION_TIMEOUT: Duration = Duration::from_secs(10);

fn build_reflection_client(is_plaintext: bool) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(REFLECTION_TIMEOUT)
        .danger_accept_invalid_certs(true);

    if is_plaintext {
        builder = builder.http2_prior_knowledge();
    }
    builder.build().context("failed to build reflection client")
}

async fn query_reflection_service(
    client: &reqwest::Client,
    base_url: &str,
    reflection_service: &str,
    request_bytes: &[u8],
) -> Result<Option<Vec<u8>>> {
    let trimmed = base_url.trim_end_matches('/');
    let url = format!(
        "{trimmed}/{reflection_service}/{}",
        reflection::REFLECTION_METHOD
    );

    let res = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/grpc")
        .header(reqwest::header::TE, "trailers")
        .body(request_bytes.to_vec())
        .send()
        .await;

    let response = match res {
        Ok(r) if r.status().is_success() => r,
        _ => return Ok(None),
    };

    let is_unimplemented = response
        .headers()
        .get("grpc-status")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s == "12");

    if is_unimplemented {
        return Ok(None);
    }

    let bytes = response.bytes().await?;
    Ok(Some(bytes.to_vec()))
}

async fn discover_symbols(
    client: &reqwest::Client,
    base_url: &str,
    reflection_service: &str,
    service_name: &str,
) -> Option<reflection::ParsedFileDescriptor> {
    let req = reflection::build_file_by_symbol_request(service_name);
    let bytes = query_reflection_service(client, base_url, reflection_service, &req)
        .await
        .ok()??;

    let (_, payload) = proto::decode_framed_grpc_message(&bytes)?;
    Some(reflection::parse_file_descriptor_proto(payload))
}

fn attach_sample_json(
    methods: &mut [GrpcMethodInfo],
    fields: &HashMap<String, Vec<reflection::ProtoFieldInfo>>,
) {
    for method in methods.iter_mut() {
        method.sample_request_json =
            Some(reflection::generate_sample_json(&method.input_type, fields));
    }
}

/// Discovers gRPC services and methods by querying Server Reflection on the upstream server.
pub async fn discover_grpc_services(target_url: &str) -> Result<GrpcReflectResponse> {
    let is_plaintext = target_url.starts_with("http://");
    let client = build_reflection_client(is_plaintext)?;
    let list_req = reflection::build_list_services_request();

    let mut reflection_ver = "v1";
    let maybe_res =
        query_reflection_service(&client, target_url, reflection::REFLECTION_V1, &list_req).await?;

    let res_bytes = match maybe_res {
        Some(b) => b,
        None => {
            reflection_ver = "v1alpha";
            query_reflection_service(
                &client,
                target_url,
                reflection::REFLECTION_V1ALPHA,
                &list_req,
            )
            .await?
            .unwrap_or_default()
        }
    };

    let service_names = reflection::parse_services_from_reflection_response(&res_bytes);
    let ref_service = if reflection_ver == "v1" {
        reflection::REFLECTION_V1
    } else {
        reflection::REFLECTION_V1ALPHA
    };

    let mut services = Vec::new();
    for svc_name in service_names {
        if let Some(parsed) = discover_symbols(&client, target_url, ref_service, &svc_name).await {
            for mut svc in parsed.services {
                attach_sample_json(&mut svc.methods, &parsed.message_fields);
                services.push(svc);
            }
        } else {
            services.push(GrpcServiceInfo {
                name: svc_name,
                methods: Vec::new(),
            });
        }
    }

    Ok(GrpcReflectResponse {
        services,
        reflection_version: reflection_ver.to_string(),
    })
}

/// Executes a gRPC unary call on the target endpoint.
pub async fn execute_grpc_call(
    target_url: &str,
    req: &GrpcInvokeRequest,
) -> Result<GrpcInvokeResponse> {
    invoke::invoke_grpc_unary(target_url, req, None).await
}
