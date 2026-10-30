//! Lightweight Protobuf wire encoder and decoder for dynamic gRPC reflection and invocation.

use bytes::Bytes;
use serde_json::{Map, Value};
use std::collections::HashMap;

const MAX_RECURSION_DEPTH: usize = 32;

/// Writes an unsigned 64-bit integer as a Protobuf varint.
pub fn write_varint(buf: &mut Vec<u8>, mut val: u64) {
    while val >= 0x80 {
        buf.push((val as u8 & 0x7f) | 0x80);
        val >>= 7;
    }
    buf.push(val as u8);
}

/// Reads a Protobuf varint from the data slice starting at `offset`.
pub fn read_varint(data: &[u8], offset: &mut usize) -> Option<u64> {
    let mut val = 0u64;
    let mut shift = 0;
    while *offset < data.len() {
        let b = data[*offset];
        *offset += 1;
        val |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(val);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

/// Writes a field tag (field number and wire type).
pub fn write_tag(buf: &mut Vec<u8>, field_number: u32, wire_type: u8) {
    write_varint(
        buf,
        ((field_number as u64) << 3) | (wire_type as u64 & 0x07),
    );
}

/// Writes length-delimited bytes (wire type 2).
pub fn write_bytes_field(buf: &mut Vec<u8>, field_number: u32, bytes: &[u8]) {
    write_tag(buf, field_number, 2);
    write_varint(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

/// Writes a string field (wire type 2).
pub fn write_string_field(buf: &mut Vec<u8>, field_number: u32, s: &str) {
    write_bytes_field(buf, field_number, s.as_bytes());
}

/// Encodes raw payload into a standard 5-byte framed gRPC message.
/// Header: [compression_flag (1 byte)][length (4 bytes BE)][payload]
pub fn encode_framed_grpc_message(payload: &[u8]) -> Bytes {
    let len = payload.len() as u32;
    let mut framed = Vec::with_capacity(5 + payload.len());
    framed.push(0x00); // 0 = uncompressed
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(payload);
    Bytes::from(framed)
}

/// Decodes a 5-byte framed gRPC message, returning (compression_flag, payload_slice).
pub fn decode_framed_grpc_message(buf: &[u8]) -> Option<(u8, &[u8])> {
    if buf.len() < 5 {
        return None;
    }
    let flag = buf[0];
    let len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    if buf.len() >= 5 + len {
        Some((flag, &buf[5..5 + len]))
    } else {
        None
    }
}

fn encode_json_number(buf: &mut Vec<u8>, tag_num: u32, num: &serde_json::Number) {
    if let Some(i) = num.as_i64() {
        write_tag(buf, tag_num, 0);
        write_varint(buf, i as u64);
    } else if let Some(u) = num.as_u64() {
        write_tag(buf, tag_num, 0);
        write_varint(buf, u);
    } else if let Some(f) = num.as_f64() {
        write_tag(buf, tag_num, 1);
        buf.extend_from_slice(&f.to_bits().to_le_bytes());
    }
}

fn encode_json_field(
    buf: &mut Vec<u8>,
    tag_num: u32,
    val: &Value,
    schema: Option<&HashMap<String, u32>>,
    depth: usize,
) {
    match val {
        Value::String(s) => write_string_field(buf, tag_num, s),
        Value::Bool(b) => {
            write_tag(buf, tag_num, 0);
            write_varint(buf, if *b { 1 } else { 0 });
        }
        Value::Number(num) => encode_json_number(buf, tag_num, num),
        Value::Object(_) => {
            if depth < MAX_RECURSION_DEPTH {
                let sub = json_to_protobuf_depth(val, schema, depth + 1);
                write_bytes_field(buf, tag_num, &sub);
            }
        }
        Value::Array(items) => {
            for item in items {
                encode_json_field(buf, tag_num, item, schema, depth);
            }
        }
        Value::Null => {}
    }
}

/// Converts a JSON Value into Protobuf wire bytes.
pub fn json_to_protobuf(val: &Value, schema: Option<&HashMap<String, u32>>) -> Vec<u8> {
    json_to_protobuf_depth(val, schema, 0)
}

fn json_to_protobuf_depth(
    val: &Value,
    schema: Option<&HashMap<String, u32>>,
    depth: usize,
) -> Vec<u8> {
    if depth >= MAX_RECURSION_DEPTH {
        return Vec::new();
    }
    let mut buf = Vec::new();
    let Value::Object(map) = val else {
        return buf;
    };

    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();

    for (idx, key) in keys.into_iter().enumerate() {
        let tag_num = schema
            .and_then(|s| s.get(key).copied())
            .or_else(|| key.parse::<u32>().ok())
            .unwrap_or((idx + 1) as u32);

        if let Some(field_val) = map.get(key) {
            encode_json_field(&mut buf, tag_num, field_val, schema, depth);
        }
    }
    buf
}

fn decode_varint_value(val: u64) -> Value {
    let signed = val as i64;
    if signed < 0 {
        Value::from(signed)
    } else {
        Value::from(val)
    }
}

fn decode_fixed64(data: &[u8], offset: &mut usize) -> Option<Value> {
    if *offset + 8 > data.len() {
        return None;
    }
    let bits = u64::from_le_bytes(data[*offset..*offset + 8].try_into().ok()?);
    *offset += 8;
    Some(Value::from(f64::from_bits(bits)))
}

fn decode_fixed32(data: &[u8], offset: &mut usize) -> Option<Value> {
    if *offset + 4 > data.len() {
        return None;
    }
    let bits = u32::from_le_bytes(data[*offset..*offset + 4].try_into().ok()?);
    *offset += 4;
    Some(Value::from(f32::from_bits(bits)))
}

fn decode_length_delimited_field(
    data: &[u8],
    offset: &mut usize,
    depth: usize,
) -> Option<Value> {
    let len = read_varint(data, offset)? as usize;
    if *offset + len > data.len() {
        return None;
    }
    let bytes = &data[*offset..*offset + len];
    *offset += len;
    Some(decode_length_delimited(bytes, depth + 1))
}

fn parse_wire_field(
    data: &[u8],
    offset: &mut usize,
    depth: usize,
) -> Option<(u32, Value)> {
    let tag = read_varint(data, offset)?;
    let field_num = (tag >> 3) as u32;
    if field_num == 0 {
        return None;
    }
    let wire_type = (tag & 0x07) as u8;
    let val = match wire_type {
        0 => decode_varint_value(read_varint(data, offset)?),
        1 => decode_fixed64(data, offset)?,
        2 => decode_length_delimited_field(data, offset, depth)?,
        5 => decode_fixed32(data, offset)?,
        _ => return None,
    };
    Some((field_num, val))
}

fn try_decode_submessage(data: &[u8], depth: usize) -> Option<Value> {
    if data.is_empty() || depth >= MAX_RECURSION_DEPTH {
        return None;
    }
    let mut map = Map::new();
    let mut offset = 0;

    while offset < data.len() {
        let (field_num, val) = parse_wire_field(data, &mut offset, depth)?;
        insert_json_field(&mut map, field_num.to_string(), val);
    }

    if offset == data.len() && !map.is_empty() {
        Some(Value::Object(map))
    } else {
        None
    }
}

fn decode_length_delimited(bytes: &[u8], depth: usize) -> Value {
    if bytes.is_empty() {
        return Value::String(String::new());
    }
    if depth < MAX_RECURSION_DEPTH && let Some(sub) = try_decode_submessage(bytes, depth) {
        return sub;
    }
    if let Ok(s) = std::str::from_utf8(bytes)
        && s.chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t')
    {
        return Value::String(s.to_string());
    }
    Value::String(hex::encode(bytes))
}

fn insert_json_field(map: &mut Map<String, Value>, key: String, val: Value) {
    if let Some(existing) = map.get_mut(&key) {
        match existing {
            Value::Array(arr) => arr.push(val),
            _ => {
                let prev = std::mem::replace(existing, Value::Null);
                *existing = Value::Array(vec![prev, val]);
            }
        }
    } else {
        map.insert(key, val);
    }
}

/// Decodes Protobuf wire bytes into a JSON Value.
pub fn protobuf_to_json(data: &[u8], schema: Option<&HashMap<u32, String>>) -> Value {
    protobuf_to_json_depth(data, schema, 0)
}

fn protobuf_to_json_depth(
    data: &[u8],
    schema: Option<&HashMap<u32, String>>,
    depth: usize,
) -> Value {
    if depth >= MAX_RECURSION_DEPTH {
        return Value::Object(Map::new());
    }
    let mut map = Map::new();
    let mut offset = 0;

    while offset < data.len() {
        let Some((field_num, val)) = parse_wire_field(data, &mut offset, depth) else {
            break;
        };
        let key = schema
            .and_then(|s| s.get(&field_num).cloned())
            .unwrap_or_else(|| field_num.to_string());
        insert_json_field(&mut map, key, val);
    }

    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_round_trip() {
        let test_cases = [0u64, 1, 127, 128, 300, 16384, u32::MAX as u64, u64::MAX];
        for &tc in &test_cases {
            let mut buf = Vec::new();
            write_varint(&mut buf, tc);
            let mut offset = 0;
            let read = read_varint(&buf, &mut offset).unwrap();
            assert_eq!(tc, read);
            assert_eq!(offset, buf.len());
        }
    }

    #[test]
    fn test_grpc_framing() {
        let payload = b"hello grpc";
        let framed = encode_framed_grpc_message(payload);
        assert_eq!(framed.len(), 5 + payload.len());
        assert_eq!(framed[0], 0x00);
        assert_eq!(&framed[1..5], &(payload.len() as u32).to_be_bytes());

        let (flag, decoded) = decode_framed_grpc_message(&framed).unwrap();
        assert_eq!(flag, 0);
        assert_eq!(decoded, payload);
    }

    #[test]
    fn test_json_to_protobuf_and_back() {
        let schema = HashMap::from([("name".to_string(), 1), ("age".to_string(), 2)]);
        let rev_schema = HashMap::from([(1, "name".to_string()), (2, "age".to_string())]);

        let input = serde_json::json!({
            "name": "Alice",
            "age": 30
        });

        let encoded = json_to_protobuf(&input, Some(&schema));
        let decoded = protobuf_to_json(&encoded, Some(&rev_schema));

        assert_eq!(decoded["name"], "Alice");
        assert_eq!(decoded["age"], 30);
    }

    #[test]
    fn test_negative_varint_round_trip() {
        let schema = HashMap::from([("count".to_string(), 1), ("offset".to_string(), 2)]);
        let rev_schema = HashMap::from([(1, "count".to_string()), (2, "offset".to_string())]);

        let input = serde_json::json!({
            "count": -42,
            "offset": -1
        });

        let encoded = json_to_protobuf(&input, Some(&schema));
        let decoded = protobuf_to_json(&encoded, Some(&rev_schema));

        assert_eq!(decoded["count"], -42);
        assert_eq!(decoded["offset"], -1);
    }

    #[test]
    fn test_submessage_and_string_discrimination() {
        let schema = HashMap::from([
            ("sub".to_string(), 1),
            ("greeting".to_string(), 2),
        ]);
        let rev_schema = HashMap::from([
            (1, "sub".to_string()),
            (2, "greeting".to_string()),
        ]);

        let input = serde_json::json!({
            "sub": {
                "id": 100,
                "label": "submessage_ok"
            },
            "greeting": "hello world"
        });

        let encoded = json_to_protobuf(&input, Some(&schema));
        let decoded = protobuf_to_json(&encoded, Some(&rev_schema));

        assert_eq!(decoded["sub"]["1"], 100);
        assert_eq!(decoded["sub"]["2"], "submessage_ok");
        assert_eq!(decoded["greeting"], "hello world");
    }

    #[test]
    fn test_recursion_depth_limit() {
        let mut deeply_nested = serde_json::json!("leaf");
        for _ in 0..50 {
            deeply_nested = serde_json::json!({ "nested": deeply_nested });
        }

        let encoded = json_to_protobuf(&deeply_nested, None);
        assert!(!encoded.is_empty());

        let decoded = protobuf_to_json(&encoded, None);
        assert!(decoded.is_object());
    }

    #[test]
    fn test_binary_hex_fallback() {
        let binary_payload = vec![0x00, 0xff, 0xaa, 0x12];
        let mut buf = Vec::new();
        write_bytes_field(&mut buf, 1, &binary_payload);

        let decoded = protobuf_to_json(&buf, None);
        assert_eq!(decoded["1"], hex::encode(&binary_payload));
    }
}
