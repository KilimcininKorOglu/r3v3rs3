//! Reads and removes the cookies of a request that only r3v3rs3 uses.

use hyper::header::{COOKIE, HeaderMap, HeaderValue};

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < bytes.len() && (bytes[start] == b' ' || bytes[start] == b'\t') {
        start += 1;
    }
    let mut end = bytes.len();
    while end > start && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t') {
        end -= 1;
    }
    &bytes[start..end]
}

fn split_cookie_pair(pair: &[u8]) -> Option<(&[u8], &[u8])> {
    let trimmed = trim_ascii(pair);
    let eq_pos = trimmed.iter().position(|&b| b == b'=')?;
    Some((&trimmed[..eq_pos], &trimmed[eq_pos + 1..]))
}

/// The values of the cookie in the `Cookie` headers, in header order.
pub fn cookie_values<'a>(headers: &'a HeaderMap, name: &'a str) -> impl Iterator<Item = &'a str> {
    let name_bytes = name.as_bytes();
    headers
        .get_all(COOKIE)
        .iter()
        .flat_map(|value| value.as_bytes().split(|&b| b == b';'))
        .filter_map(split_cookie_pair)
        .filter(move |(pair_name, _)| *pair_name == name_bytes)
        .filter_map(|(_, value)| std::str::from_utf8(value).ok())
}

/// Removes the cookie from the `Cookie` headers, so the upstream server does not receive it.
pub fn remove_cookie(headers: &mut HeaderMap, name: &str) {
    let values = headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| without_cookie(value, name))
        .collect::<Vec<_>>();
    headers.remove(COOKIE);
    for value in values {
        headers.append(COOKIE, value);
    }
}

fn join_cookie_parts(parts: &[&[u8]]) -> Vec<u8> {
    let mut joined = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            joined.extend_from_slice(b"; ");
        }
        joined.extend_from_slice(part);
    }
    joined
}

/// Returns the `Cookie` header value without the cookie, or `None` when no cookie is left. A
/// header with non-ASCII bytes in other cookies keeps those cookies intact.
fn without_cookie(value: &HeaderValue, name: &str) -> Option<HeaderValue> {
    let name_bytes = name.as_bytes();
    let mut remaining: Vec<&[u8]> = Vec::new();
    for part in value.as_bytes().split(|&b| b == b';') {
        let trimmed = trim_ascii(part);
        if trimmed.is_empty() {
            continue;
        }
        if let Some((k, _)) = split_cookie_pair(trimmed)
            && k == name_bytes
        {
            continue;
        }
        remaining.push(trimmed);
    }
    if remaining.is_empty() {
        return None;
    }
    HeaderValue::from_bytes(&join_cookie_parts(&remaining)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cookie_is_found_in_every_header_and_removed() {
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, HeaderValue::from_static("theme=dark; id=abc; flag"));
        headers.append(COOKIE, HeaderValue::from_static("id=def"));
        headers.append(COOKIE, HeaderValue::from_static("lang=tr"));
        let values = cookie_values(&headers, "id").collect::<Vec<_>>();
        assert_eq!(values, ["abc", "def"]);

        remove_cookie(&mut headers, "id");
        assert_eq!(
            headers.get_all(COOKIE).iter().collect::<Vec<_>>(),
            ["theme=dark; flag", "lang=tr"]
        );
        assert_eq!(cookie_values(&headers, "id").next(), None);
    }

    #[test]
    fn non_ascii_cookie_does_not_hide_proxy_cookies() {
        let mut headers = HeaderMap::new();
        let raw = b"user=\xe4\xb8\xad; id=secret-123; flag=\xff";
        headers.append(COOKIE, HeaderValue::from_bytes(raw).unwrap());

        let values = cookie_values(&headers, "id").collect::<Vec<_>>();
        assert_eq!(values, ["secret-123"]);

        remove_cookie(&mut headers, "id");
        let remaining = headers.get(COOKIE).unwrap().as_bytes();
        assert_eq!(remaining, b"user=\xe4\xb8\xad; flag=\xff");
        assert_eq!(cookie_values(&headers, "id").next(), None);
    }
}
