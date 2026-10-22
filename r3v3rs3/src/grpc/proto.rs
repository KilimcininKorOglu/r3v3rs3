//! Lightweight Protobuf wire encoder and decoder for dynamic gRPC reflection and invocation.

use bytes::Bytes;
use serde_json::{Map, Value};
use std::collections::HashMap;

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

fn encode_json_field(
    buf: &mut Vec<u8>,
    tag_num: u32,
    val: &Value,
    schema: Option<&HashMap<String, u32>>,
) {
    match val {
        Value::String(s) => write_string_field(buf, tag_num, s),
        Value::Bool(b) => {
            write_tag(buf, tag_num, 0);
            write_varint(buf, if *b { 1 } else { 0 });
        }
        Value::Number(num) => {
            if let Some(i) = num.as_i64() {
                write_tag(buf, tag_num, 0);
                write_varint(buf, i as u64);
            } else if let Some(u) = num.as_u64() {
                write_tag(buf, tag_num, 0);
                write_varint(buf, u);
            } else if let Some(f) = num.as_f64() {
                write_tag(buf, tag_num, 1); // 64-bit double
                buf.extend_from_slice(&f.to_bits().to_le_bytes());
            }
        }
        Value::Object(_) => {
            let sub = json_to_protobuf(val, schema);
            write_bytes_field(buf, tag_num, &sub);
        }
        Value::Array(items) => {
            for item in items {
                encode_json_field(buf, tag_num, item, schema);
            }
        }
        Value::Null => {}
    }
}

/// Converts a JSON Value into Protobuf wire bytes.
pub fn json_to_protobuf(val: &Value, schema: Option<&HashMap<String, u32>>) -> Vec<u8> {
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
            encode_json_field(&mut buf, tag_num, field_val, schema);
        }
    }
    buf
}

fn decode_length_delimited(bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        return Value::String(String::new());
    }
    if let Ok(s) = std::str::from_utf8(bytes)
        && s.chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t')
    {
        return Value::String(s.to_string());
    }
    let sub = protobuf_to_json(bytes, None);
    if let Value::Object(sub_map) = &sub
        && !sub_map.is_empty()
    {
        return sub;
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
    let mut map = Map::new();
    let mut offset = 0;

    while offset < data.len() {
        let Some(tag_varint) = read_varint(data, &mut offset) else {
            break;
        };
        let field_num = (tag_varint >> 3) as u32;
        let wire_type = (tag_varint & 0x07) as u8;

        let key = schema
            .and_then(|s| s.get(&field_num).cloned())
            .unwrap_or_else(|| field_num.to_string());

        match wire_type {
            0 => {
                let Some(val) = read_varint(data, &mut offset) else {
                    break;
                };
                insert_json_field(&mut map, key, Value::from(val));
            }
            1 => {
                if offset + 8 > data.len() {
                    break;
                }
                let bits = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
                offset += 8;
                insert_json_field(&mut map, key, Value::from(f64::from_bits(bits)));
            }
            2 => {
                let Some(len) = read_varint(data, &mut offset) else {
                    break;
                };
                let len = len as usize;
                if offset + len > data.len() {
                    break;
                }
                let bytes = &data[offset..offset + len];
                offset += len;
                insert_json_field(&mut map, key, decode_length_delimited(bytes));
            }
            5 => {
                if offset + 4 > data.len() {
                    break;
                }
                let bits = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                insert_json_field(&mut map, key, Value::from(f32::from_bits(bits)));
            }
            _ => break,
        }
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
}
