//! Answers a request with the first redirect rule of its proxy that matches it.

use bytes::Bytes;
use http_body_util::Full;
use hyper::header::{HeaderValue, LOCATION};
use hyper::{Request, Response};
use r3v3rs3_api::redirect::RedirectRule;
use tracing::warn;

/// Builds the redirect of the first rule whose regex matches `host/path?query`. A target that
/// becomes an invalid header value does not match, so a later rule can match.
pub fn redirect_rule<B>(
    rules: &[RedirectRule],
    host: Option<&str>,
    req: &Request<B>,
) -> Option<Response<Full<Bytes>>> {
    if rules.is_empty() {
        return None;
    }
    let path = req.uri().path_and_query().map_or("/", |path| path.as_str());
    let subject = format!("{}{path}", host.unwrap_or_default());
    rules.iter().find_map(|rule| {
        let location = rule.location(&subject)?;
        let value = HeaderValue::from_str(&location)
            .inspect_err(
                |err| warn!(%err, location, "the redirect target is not a valid header value"),
            )
            .ok()?;
        Response::builder()
            .status(rule.status.code())
            .header(LOCATION, value)
            .body(Full::new(Bytes::new()))
            .ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(lines: &[&str]) -> Vec<RedirectRule> {
        lines
            .iter()
            .map(|line| RedirectRule::parse_line(line).unwrap())
            .collect()
    }

    fn redirect(rules: &[RedirectRule], host: &str, path: &str) -> Option<(u16, String)> {
        let req = Request::get(path).body(()).unwrap();
        let res = redirect_rule(rules, Some(host), &req)?;
        let location = res.headers()[LOCATION].to_str().unwrap().to_string();
        Some((res.status().as_u16(), location))
    }

    #[test]
    fn the_first_matching_rule_answers() {
        let rules = rules(&[
            r"308 ^a\.test/x$ https://first.test/",
            r"301 ^a\.test/(.*)$ https://second.test/${1}",
        ]);
        let first = (308, "https://first.test/".to_string());
        assert_eq!(redirect(&rules, "a.test", "/x"), Some(first));
        let second = (301, "https://second.test/y?z=1".to_string());
        assert_eq!(redirect(&rules, "a.test", "/y?z=1"), Some(second));
        assert_eq!(redirect(&rules, "b.test", "/x"), None);
        assert_eq!(redirect(&[], "a.test", "/x"), None);
    }

    #[test]
    fn a_target_that_is_not_a_header_value_does_not_match() {
        let rules = rules(&[
            r"301 ^(.*)/bad$ https://${1}/",
            r"302 /bad$ https://fallback.test/",
        ]);
        let fallback = (302, "https://fallback.test/".to_string());
        assert_eq!(redirect(&rules, "a\u{7f}", "/bad"), Some(fallback));
    }
}
