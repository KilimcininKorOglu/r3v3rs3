use hyper::Request;
use r3v3rs3_api::proxy::Route;
use r3v3rs3_api::subject_name::SubjectName;
use r3v3rs3_api::vhost::VirtualHost;

#[derive(Debug, Default)]
pub struct RequestFilter {
    pub vhosts: Vec<VirtualHost>,
    pub path: Vec<String>,
}

/// How a virtual host of a route matches the request host. A later variant is more specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum HostMatch {
    /// The route has no virtual host, so it accepts every host.
    Any,
    Regex,
    Wildcard,
    /// A DNS name or an IP address that is equal to the request host.
    Exact,
}

/// How specifically a route matches a request. The more specific host match wins, then the
/// longer path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchRank {
    host: HostMatch,
    path_segments: usize,
}

impl RequestFilter {
    pub fn new(vhosts: &[VirtualHost], route: &Route) -> Self {
        Self {
            vhosts: vhosts.to_vec(),
            path: route
                .path
                .split('/')
                .filter(|seg| !seg.is_empty())
                .map(|s| s.to_owned())
                .collect(),
        }
    }

    /// Returns the rank of the route for the request, or `None` when the route does not match.
    pub fn rank<T>(&self, req: &Request<T>, host: Option<&str>) -> Option<MatchRank> {
        let host = self.host_match(host)?;
        let path = normalize_path(req.uri().path());
        let matched = request_segments(&path)
            .zip(self.path.iter())
            .take_while(|(a, b)| a == b)
            .count();
        (matched == self.path.len()).then_some(MatchRank {
            host,
            path_segments: matched,
        })
    }

    /// Returns the path segments of the request after the path of the route. Call it only for a
    /// route whose [`RequestFilter::rank`] is `Some`.
    pub fn result<T>(&self, req: &Request<T>) -> FilterResult {
        let path = normalize_path(req.uri().path());
        FilterResult {
            path_segments: request_segments(&path)
                .skip(self.path.len())
                .map(str::to_string)
                .collect(),
        }
    }

    pub fn matches_host(&self, host: Option<&str>) -> bool {
        self.host_match(host).is_some()
    }

    fn host_match(&self, host: Option<&str>) -> Option<HostMatch> {
        if self.vhosts.is_empty() {
            return Some(HostMatch::Any);
        }
        let host = host?;
        self.vhosts
            .iter()
            .filter(|vhost| vhost.test(host))
            .map(vhost_match)
            .max()
    }
}

fn vhost_match(vhost: &VirtualHost) -> HostMatch {
    match vhost {
        VirtualHost::SubjectName(SubjectName::WildcardDnsName(_)) => HostMatch::Wildcard,
        VirtualHost::SubjectName(_) => HostMatch::Exact,
        VirtualHost::Regex(_) => HostMatch::Regex,
    }
}

fn is_single_dot(segment: &str) -> bool {
    segment.eq_ignore_ascii_case(".") || segment.eq_ignore_ascii_case("%2e")
}

fn is_double_dot(segment: &str) -> bool {
    matches!(
        segment.to_ascii_lowercase().as_str(),
        ".." | ".%2e" | "%2e." | "%2e%2e"
    )
}

fn needs_trailing_slash(path: &str, last_was_dot: bool, result: &str) -> bool {
    (path.ends_with('/') || last_was_dot) && !result.ends_with('/')
}

/// Resolves `.` and `..` (including percent-encoded `%2e`) segments according to RFC 3986.
pub fn normalize_path(path: &str) -> String {
    let mut normalized = Vec::new();
    let mut last_was_dot = false;
    for segment in path.split('/') {
        if segment.is_empty() {
            continue;
        }
        if is_single_dot(segment) {
            last_was_dot = true;
            continue;
        }
        if is_double_dot(segment) {
            normalized.pop();
            last_was_dot = true;
            continue;
        }
        last_was_dot = false;
        normalized.push(segment);
    }
    let mut result = format!("/{}", normalized.join("/"));
    if needs_trailing_slash(path, last_was_dot, &result) {
        result.push('/');
    }
    result
}

fn request_segments(path: &str) -> std::str::Split<'_, char> {
    path.trim_start_matches('/').split('/')
}

#[derive(Debug)]
pub struct FilterResult {
    pub path_segments: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(vhosts: &[&str], path: &str) -> RequestFilter {
        let vhosts = vhosts
            .iter()
            .map(|vhost| vhost.parse().unwrap())
            .collect::<Vec<_>>();
        let route = Route {
            path: path.into(),
            ..Default::default()
        };
        RequestFilter::new(&vhosts, &route)
    }

    fn request(path: &str) -> Request<()> {
        Request::get(path).body(()).unwrap()
    }

    #[test]
    fn a_longer_path_ranks_higher() {
        let req = request("/api/users");
        let root = filter(&[], "/").rank(&req, None).unwrap();
        let api = filter(&[], "/api/").rank(&req, None).unwrap();
        assert!(api > root);
        assert_eq!(filter(&[], "/apiv2").rank(&req, None), None);
        assert_eq!(
            filter(&[], "/api").result(&req).path_segments,
            vec!["users".to_string()]
        );
    }

    #[test]
    fn a_more_specific_host_ranks_higher_than_a_longer_path() {
        let req = request("/api");
        let host = Some("app.example.com");
        let ranks = [
            filter(&[], "/api").rank(&req, host),
            filter(&["^app\\..*$"], "/api").rank(&req, host),
            filter(&["*.example.com"], "/").rank(&req, host),
            filter(&["other.org", "app.example.com"], "/").rank(&req, host),
        ];
        let ranks = ranks.map(Option::unwrap);
        assert!(ranks.windows(2).all(|pair| pair[0] < pair[1]), "{ranks:?}");
    }

    #[test]
    fn a_route_with_a_virtual_host_needs_a_matching_host() {
        let req = request("/");
        assert_eq!(filter(&["example.com"], "/").rank(&req, None), None);
        assert_eq!(
            filter(&["example.com"], "/").rank(&req, Some("other.org")),
            None
        );
        assert!(filter(&[], "/").rank(&req, None).is_some());
    }

    #[test]
    fn dot_segments_are_normalized() {
        let req1 = request("/./admin");
        let req2 = request("/%2e/admin");
        let req3 = request("/foo/../admin");
        let req4 = request("/foo/%2e%2e/admin");
        let admin_filter = filter(&[], "/admin");

        assert!(admin_filter.rank(&req1, None).is_some());
        assert!(admin_filter.rank(&req2, None).is_some());
        assert!(admin_filter.rank(&req3, None).is_some());
        assert!(admin_filter.rank(&req4, None).is_some());

        assert_eq!(admin_filter.result(&req1).path_segments, Vec::<String>::new());
        assert_eq!(admin_filter.result(&req2).path_segments, Vec::<String>::new());
        assert_eq!(admin_filter.result(&req3).path_segments, Vec::<String>::new());
        assert_eq!(admin_filter.result(&req4).path_segments, Vec::<String>::new());

        let req_sub = request("/./admin/users");
        assert_eq!(
            admin_filter.result(&req_sub).path_segments,
            vec!["users".to_string()]
        );
    }
}
