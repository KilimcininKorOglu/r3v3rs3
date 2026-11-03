use hyper::header::{HeaderMap, HeaderName, HeaderValue};

pub const TRACEPARENT: &str = "traceparent";
#[allow(dead_code)]
pub const TRACESTATE: &str = "tracestate";
pub const X_REQUEST_ID: &str = "x-request-id";

/// Parsed representation of a W3C Trace Context `traceparent` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceParent {
    pub version: String,
    pub trace_id: String,
    pub parent_id: String,
    pub flags: String,
}

fn is_valid_hex(s: &str, expected_len: usize) -> bool {
    s.len() == expected_len && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_non_zero_hex(s: &str, expected_len: usize) -> bool {
    is_valid_hex(s, expected_len) && !s.chars().all(|c| c == '0')
}

fn validate_traceparent_parts(parts: &[&str]) -> bool {
    if parts.len() < 4 {
        return false;
    }
    let version = parts[0];
    if version == "ff" || !is_valid_hex(version, 2) {
        return false;
    }
    if version == "00" && parts.len() != 4 {
        return false;
    }
    if !is_non_zero_hex(parts[1], 32) {
        return false;
    }
    if !is_non_zero_hex(parts[2], 16) {
        return false;
    }
    is_valid_hex(parts[3], 2)
}

impl TraceParent {
    /// Parses and validates a W3C `traceparent` header value according to the recommendation.
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.trim().split('-').collect();
        if !validate_traceparent_parts(&parts) {
            return None;
        }
        Some(Self {
            version: parts[0].to_ascii_lowercase(),
            trace_id: parts[1].to_ascii_lowercase(),
            parent_id: parts[2].to_ascii_lowercase(),
            flags: parts[3].to_ascii_lowercase(),
        })
    }

    /// Generates a child `traceparent` header value for forwarding downstream with a newly
    /// generated 16-hex child span ID while preserving the original trace ID and flags.
    pub fn child_traceparent(&self) -> String {
        let child_span_id = format!("{:016x}", rand::random::<u64>());
        format!(
            "{}-{}-{}-{}",
            self.version, self.trace_id, child_span_id, self.flags
        )
    }

    /// Generates a new root `traceparent` header with version `00`, the provided 32-hex trace ID,
    /// a newly generated 16-hex span ID, and sampled flag `01`.
    pub fn new_root(trace_id: &str) -> String {
        let span_id = format!("{:016x}", rand::random::<u64>());
        format!("00-{trace_id}-{span_id}-01")
    }
}

/// Resolves the correlation/request ID from request headers. Checks `x-request-id` first,
/// then the trace ID from a valid `traceparent`, and falls back to a new 32-hex random ID.
pub fn resolve_request_id(headers: &HeaderMap) -> String {
    if let Some(val) = headers.get(X_REQUEST_ID)
        && let Ok(s) = val.to_str()
        && !s.trim().is_empty()
    {
        return s.trim().to_string();
    }
    if let Some(val) = headers.get(TRACEPARENT)
        && let Ok(s) = val.to_str()
        && let Some(tp) = TraceParent::parse(s)
    {
        return tp.trace_id;
    }
    super::header_rules::new_request_id()
}

/// Extracts incoming W3C trace context, updates or injects `traceparent` and `x-request-id`
/// headers for upstream request propagation, and returns the resolved request ID.
pub fn extract_and_propagate_trace_context(headers: &mut HeaderMap) -> String {
    let incoming_tp = headers
        .get(TRACEPARENT)
        .and_then(|v| v.to_str().ok())
        .and_then(TraceParent::parse);

    let (trace_id, outbound_tp) = match incoming_tp {
        Some(tp) => {
            let outbound = tp.child_traceparent();
            (tp.trace_id, outbound)
        }
        None => {
            let trace_id = super::header_rules::new_request_id();
            let outbound = TraceParent::new_root(&trace_id);
            (trace_id, outbound)
        }
    };

    if let Ok(val) = HeaderValue::from_str(&outbound_tp) {
        headers.insert(HeaderName::from_static(TRACEPARENT), val);
    }

    let request_id = match headers.get(X_REQUEST_ID).and_then(|v| v.to_str().ok()) {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => {
            if let Ok(val) = HeaderValue::from_str(&trace_id) {
                headers.insert(HeaderName::from_static(X_REQUEST_ID), val);
            }
            trace_id
        }
    };

    request_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_w3c_traceparent() {
        let raw = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let tp = TraceParent::parse(raw).expect("must parse valid traceparent");
        assert_eq!(tp.version, "00");
        assert_eq!(tp.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(tp.parent_id, "00f067aa0ba902b7");
        assert_eq!(tp.flags, "01");

        let child = tp.child_traceparent();
        let child_tp = TraceParent::parse(&child).expect("child must be valid");
        assert_eq!(child_tp.version, "00");
        assert_eq!(child_tp.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(child_tp.flags, "01");
        assert_ne!(child_tp.parent_id, "00f067aa0ba902b7");
    }

    #[test]
    fn reject_invalid_traceparent() {
        // Version ff is invalid
        assert!(
            TraceParent::parse("ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").is_none()
        );
        // All-zero trace ID is invalid
        assert!(
            TraceParent::parse("00-00000000000000000000000000000000-00f067aa0ba902b7-01").is_none()
        );
        // All-zero parent ID is invalid
        assert!(
            TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01").is_none()
        );
        // Short trace ID is invalid
        assert!(
            TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01").is_none()
        );
        // Version 00 with extra parts
        assert!(
            TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra")
                .is_none()
        );
    }

    #[test]
    fn extract_and_propagate_generates_and_attaches_headers() {
        let mut headers = HeaderMap::new();
        let req_id = extract_and_propagate_trace_context(&mut headers);
        assert!(!req_id.is_empty());
        assert!(headers.contains_key(TRACEPARENT));
        assert_eq!(
            headers.get(X_REQUEST_ID).and_then(|v| v.to_str().ok()),
            Some(req_id.as_str())
        );
        let tp = TraceParent::parse(headers.get(TRACEPARENT).unwrap().to_str().unwrap()).unwrap();
        assert_eq!(tp.trace_id, req_id);
    }

    #[test]
    fn extract_and_propagate_propagates_inbound_traceparent() {
        let mut headers = HeaderMap::new();
        let inbound = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        headers.insert(TRACEPARENT, HeaderValue::from_static(inbound));
        let req_id = extract_and_propagate_trace_context(&mut headers);
        assert_eq!(req_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        let outbound_tp =
            TraceParent::parse(headers.get(TRACEPARENT).unwrap().to_str().unwrap()).unwrap();
        assert_eq!(outbound_tp.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_ne!(outbound_tp.parent_id, "00f067aa0ba902b7");
        assert_eq!(
            headers.get(X_REQUEST_ID).and_then(|v| v.to_str().ok()),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
    }

    #[test]
    fn extract_and_propagate_preserves_tracestate() {
        let mut headers = HeaderMap::new();
        headers.insert(TRACESTATE, HeaderValue::from_static("rojo=1,congo=2"));
        let _ = extract_and_propagate_trace_context(&mut headers);
        assert_eq!(
            headers.get(TRACESTATE).and_then(|v| v.to_str().ok()),
            Some("rojo=1,congo=2")
        );
    }
}
