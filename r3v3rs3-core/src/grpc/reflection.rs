//! Client queries and parsers for the standard gRPC Server Reflection protocol.

use super::proto::{
    decode_framed_grpc_message, encode_framed_grpc_message, read_varint, write_string_field,
};
use bytes::Bytes;
use r3v3rs3_api::grpc::{GrpcMethodInfo, GrpcServiceInfo};
use std::collections::HashMap;

pub const REFLECTION_V1: &str = "grpc.reflection.v1.ServerReflection";
pub const REFLECTION_V1ALPHA: &str = "grpc.reflection.v1alpha.ServerReflection";
pub const REFLECTION_METHOD: &str = "ServerReflectionInfo";

/// Encodes a `ServerReflectionRequest` requesting all service names (`list_services = "*"`).
pub fn build_list_services_request() -> Bytes {
    let mut payload = Vec::new();
    write_string_field(&mut payload, 7, "*");
    encode_framed_grpc_message(&payload)
}

/// Encodes a `ServerReflectionRequest` requesting descriptors containing `symbol`.
pub fn build_file_by_symbol_request(symbol: &str) -> Bytes {
    let mut payload = Vec::new();
    write_string_field(&mut payload, 4, symbol);
    encode_framed_grpc_message(&payload)
}

fn read_string_field_at(data: &[u8], offset: &mut usize) -> Option<String> {
    let len = read_varint(data, offset)? as usize;
    if *offset + len <= data.len() {
        let s = std::str::from_utf8(&data[*offset..*offset + len])
            .ok()?
            .to_string();
        *offset += len;
        Some(s)
    } else {
        None
    }
}

fn parse_service_response(data: &[u8]) -> Option<String> {
    let mut offset = 0;
    while offset < data.len() {
        let tag = read_varint(data, &mut offset)?;
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x07) as u8;

        if field_num == 1 && wire_type == 2 {
            return read_string_field_at(data, &mut offset);
        }
        skip_field(data, &mut offset, wire_type)?;
    }
    None
}

fn parse_list_services_response(data: &[u8]) -> Vec<String> {
    let mut services = Vec::new();
    let mut offset = 0;

    while offset < data.len() {
        let Some(tag) = read_varint(data, &mut offset) else {
            break;
        };
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x07) as u8;

        if field_num == 1 && wire_type == 2 {
            let Some(len) = read_varint(data, &mut offset) else {
                break;
            };
            let len = len as usize;
            if offset + len > data.len() {
                break;
            }
            if let Some(svc) = parse_service_response(&data[offset..offset + len])
                && svc != REFLECTION_V1
                && svc != REFLECTION_V1ALPHA
            {
                services.push(svc);
            }
            offset += len;
        } else if !skip_field_bool(data, &mut offset, wire_type) {
            break;
        }
    }
    services
}

/// Parses service names from a framed gRPC `ServerReflectionResponse`.
pub fn parse_services_from_reflection_response(buf: &[u8]) -> Vec<String> {
    let Some((_, payload)) = decode_framed_grpc_message(buf) else {
        return Vec::new();
    };

    let mut offset = 0;
    while offset < payload.len() {
        let Some(tag) = read_varint(payload, &mut offset) else {
            break;
        };
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x07) as u8;

        if field_num == 5 && wire_type == 2 {
            let Some(len) = read_varint(payload, &mut offset) else {
                break;
            };
            let len = len as usize;
            if offset + len <= payload.len() {
                return parse_list_services_response(&payload[offset..offset + len]);
            }
            break;
        } else if !skip_field_bool(payload, &mut offset, wire_type) {
            break;
        }
    }
    Vec::new()
}

fn skip_field_bool(data: &[u8], offset: &mut usize, wire_type: u8) -> bool {
    skip_field(data, offset, wire_type).is_some()
}

fn skip_field(data: &[u8], offset: &mut usize, wire_type: u8) -> Option<()> {
    match wire_type {
        0 => {
            read_varint(data, offset)?;
            Some(())
        }
        1 => {
            if *offset + 8 <= data.len() {
                *offset += 8;
                Some(())
            } else {
                None
            }
        }
        2 => {
            let len = read_varint(data, offset)? as usize;
            if *offset + len <= data.len() {
                *offset += len;
                Some(())
            } else {
                None
            }
        }
        5 => {
            if *offset + 4 <= data.len() {
                *offset += 4;
                Some(())
            } else {
                None
            }
        }
        _ => None,
    }
}

pub type ProtoFieldInfo = (String, u32, String);
pub type ProtoMessageSchema = (String, Vec<ProtoFieldInfo>);

/// Structure extracted from a `FileDescriptorProto`.
#[derive(Debug, Default)]
pub struct ParsedFileDescriptor {
    pub package: String,
    pub services: Vec<GrpcServiceInfo>,
    pub message_fields: HashMap<String, Vec<ProtoFieldInfo>>,
}

#[derive(Default)]
struct MethodFields {
    name: String,
    input_type: String,
    output_type: String,
    client_streaming: bool,
    server_streaming: bool,
}

fn handle_method_field(
    data: &[u8],
    offset: &mut usize,
    tag: u64,
    fields: &mut MethodFields,
) -> Option<()> {
    let field_num = (tag >> 3) as u32;
    let wire_type = (tag & 0x07) as u8;
    match (field_num, wire_type) {
        (1, 2) => fields.name = read_string_field_at(data, offset)?,
        (2, 2) => fields.input_type = read_string_field_at(data, offset)?,
        (3, 2) => fields.output_type = read_string_field_at(data, offset)?,
        (5, 0) => fields.client_streaming = read_varint(data, offset)? != 0,
        (6, 0) => fields.server_streaming = read_varint(data, offset)? != 0,
        _ => skip_field(data, offset, wire_type)?,
    }
    Some(())
}

fn build_method_info(fields: MethodFields, package: &str, service: &str) -> Option<GrpcMethodInfo> {
    if fields.name.is_empty() {
        return None;
    }
    let full_service = if package.is_empty() {
        service.to_string()
    } else {
        format!("{package}.{service}")
    };
    let full_name = format!("/{full_service}/{}", fields.name);

    Some(GrpcMethodInfo {
        name: fields.name,
        full_name,
        input_type: fields.input_type,
        output_type: fields.output_type,
        client_streaming: fields.client_streaming,
        server_streaming: fields.server_streaming,
        sample_request_json: None,
    })
}

fn parse_method_descriptor(data: &[u8], package: &str, service: &str) -> Option<GrpcMethodInfo> {
    let mut fields = MethodFields::default();
    let mut offset = 0;
    while offset < data.len() {
        let tag = read_varint(data, &mut offset)?;
        handle_method_field(data, &mut offset, tag, &mut fields)?;
    }
    build_method_info(fields, package, service)
}

fn handle_service_field(
    data: &[u8],
    offset: &mut usize,
    tag: u64,
    package: &str,
    name: &mut String,
    methods: &mut Vec<GrpcMethodInfo>,
) -> Option<()> {
    let field_num = (tag >> 3) as u32;
    let wire_type = (tag & 0x07) as u8;
    match (field_num, wire_type) {
        (1, 2) => *name = read_string_field_at(data, offset)?,
        (2, 2) => {
            let len = read_varint(data, offset)? as usize;
            if *offset + len <= data.len() {
                if let Some(m) =
                    parse_method_descriptor(&data[*offset..*offset + len], package, name)
                {
                    methods.push(m);
                }
                *offset += len;
            }
        }
        _ => skip_field(data, offset, wire_type)?,
    }
    Some(())
}

fn parse_service_descriptor(data: &[u8], package: &str) -> Option<GrpcServiceInfo> {
    let mut name = String::new();
    let mut methods = Vec::new();

    let mut offset = 0;
    while offset < data.len() {
        let tag = read_varint(data, &mut offset)?;
        handle_service_field(data, &mut offset, tag, package, &mut name, &mut methods)?;
    }

    if name.is_empty() {
        return None;
    }

    let full_name = if package.is_empty() {
        name
    } else {
        format!("{package}.{name}")
    };
    Some(GrpcServiceInfo {
        name: full_name,
        methods,
    })
}

fn handle_field_tag(
    data: &[u8],
    offset: &mut usize,
    tag: u64,
    name: &mut String,
    number: &mut u32,
    type_name: &mut String,
) -> Option<()> {
    let field_num = (tag >> 3) as u32;
    let wire_type = (tag & 0x07) as u8;
    match (field_num, wire_type) {
        (1, 2) => *name = read_string_field_at(data, offset)?,
        (3, 0) => *number = read_varint(data, offset)? as u32,
        (6, 2) => *type_name = read_string_field_at(data, offset)?,
        _ => skip_field(data, offset, wire_type)?,
    }
    Some(())
}

fn parse_field_descriptor(data: &[u8]) -> Option<ProtoFieldInfo> {
    let mut name = String::new();
    let mut number = 0u32;
    let mut type_name = String::new();

    let mut offset = 0;
    while offset < data.len() {
        let tag = read_varint(data, &mut offset)?;
        handle_field_tag(
            data,
            &mut offset,
            tag,
            &mut name,
            &mut number,
            &mut type_name,
        )?;
    }

    if name.is_empty() {
        None
    } else {
        Some((name, number, type_name))
    }
}

fn parse_message_descriptor(data: &[u8]) -> Option<ProtoMessageSchema> {
    let mut name = String::new();
    let mut fields = Vec::new();

    let mut offset = 0;
    while offset < data.len() {
        let tag = read_varint(data, &mut offset)?;
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x07) as u8;

        match (field_num, wire_type) {
            (1, 2) => name = read_string_field_at(data, &mut offset)?,
            (2, 2) => {
                let len = read_varint(data, &mut offset)? as usize;
                if offset + len <= data.len() {
                    if let Some(f) = parse_field_descriptor(&data[offset..offset + len]) {
                        fields.push(f);
                    }
                    offset += len;
                }
            }
            _ => skip_field(data, &mut offset, wire_type)?,
        }
    }

    if name.is_empty() {
        None
    } else {
        Some((name, fields))
    }
}

fn handle_file_proto_item(
    data: &[u8],
    offset: &mut usize,
    tag: u64,
    result: &mut ParsedFileDescriptor,
) -> Option<()> {
    let field_num = (tag >> 3) as u32;
    let wire_type = (tag & 0x07) as u8;

    match (field_num, wire_type) {
        (2, 2) => {
            result.package = read_string_field_at(data, offset)?;
        }
        (4, 2) => {
            let len = read_varint(data, offset)? as usize;
            if *offset + len <= data.len() {
                if let Some((msg, flds)) = parse_message_descriptor(&data[*offset..*offset + len]) {
                    result.message_fields.insert(msg, flds);
                }
                *offset += len;
            }
        }
        (6, 2) => {
            let len = read_varint(data, offset)? as usize;
            if *offset + len <= data.len() {
                if let Some(svc) =
                    parse_service_descriptor(&data[*offset..*offset + len], &result.package)
                {
                    result.services.push(svc);
                }
                *offset += len;
            }
        }
        _ => skip_field(data, offset, wire_type)?,
    }
    Some(())
}

/// Parses a serialized `FileDescriptorProto` into services and message field definitions.
pub fn parse_file_descriptor_proto(data: &[u8]) -> ParsedFileDescriptor {
    let mut result = ParsedFileDescriptor::default();
    let mut offset = 0;

    while offset < data.len() {
        let Some(tag) = read_varint(data, &mut offset) else {
            break;
        };
        if handle_file_proto_item(data, &mut offset, tag, &mut result).is_none() {
            break;
        }
    }

    result
}

/// Generates a sample JSON template for a given input message type name.
pub fn generate_sample_json(
    input_type: &str,
    message_fields: &HashMap<String, Vec<ProtoFieldInfo>>,
) -> String {
    let type_key = input_type.rsplit('.').next().unwrap_or(input_type);
    if let Some(fields) = message_fields.get(type_key) {
        let mut map = serde_json::Map::new();
        for (field_name, _, _) in fields {
            map.insert(
                field_name.clone(),
                serde_json::Value::String("example".to_string()),
            );
        }
        serde_json::to_string_pretty(&serde_json::Value::Object(map))
            .unwrap_or_else(|_| "{}".to_string())
    } else {
        "{}".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_list_services_request() {
        let req = build_list_services_request();
        assert!(req.len() >= 5);
        let (flag, payload) = decode_framed_grpc_message(&req).unwrap();
        assert_eq!(flag, 0);
        assert_eq!(payload, &[0x3a, 0x01, b'*']);
    }

    #[test]
    fn test_build_file_by_symbol_request() {
        let req = build_file_by_symbol_request("helloworld.Greeter");
        let (_, payload) = decode_framed_grpc_message(&req).unwrap();
        assert_eq!(payload[0], (4 << 3) | 2);
    }
}
