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
        let matched = request_segments(req)
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
        FilterResult {
            path_segments: request_segments(req)
                .skip(self.path.len())
                .map(str::to_string)
                .collect(),
        }
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

fn request_segments<T>(req: &Request<T>) -> std::str::Split<'_, char> {
    req.uri().path().trim_start_matches('/').split('/')
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
            servers: vec![],
            ip_filter: None,
            rate_limit: None,
            auth: None,
            headers: None,
            timeouts: None,
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
}
