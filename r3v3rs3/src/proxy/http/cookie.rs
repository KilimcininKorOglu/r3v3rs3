//! Reads and removes the cookies of a request that only r3v3rs3 uses.

use hyper::header::{COOKIE, HeaderMap, HeaderValue};

/// The values of the cookie in the `Cookie` headers, in header order.
pub fn cookie_values<'a>(headers: &'a HeaderMap, name: &'a str) -> impl Iterator<Item = &'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(move |(pair_name, _)| *pair_name == name)
        .map(|(_, value)| value)
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

/// Returns the `Cookie` header value without the cookie, or `None` when no cookie is left. A
/// value that is not visible ASCII cannot carry the cookie and is kept.
fn without_cookie(value: &HeaderValue, name: &str) -> Option<HeaderValue> {
    let Ok(text) = value.to_str() else {
        return Some(value.clone());
    };
    let rest = text
        .split(';')
        .map(str::trim)
        .filter(|pair| {
            !pair.is_empty()
                && pair
                    .split_once('=')
                    .is_none_or(|(pair_name, _)| pair_name != name)
        })
        .collect::<Vec<_>>()
        .join("; ");
    if rest.is_empty() {
        return None;
    }
    // The pairs are parts of a visible ASCII value, so the joined value is valid.
    HeaderValue::from_str(&rest).ok()
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
}
