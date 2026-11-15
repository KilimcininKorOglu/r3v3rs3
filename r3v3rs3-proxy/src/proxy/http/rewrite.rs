//! Builds the upstream path segments of a request from the path rewrite of its route.

use r3v3rs3_api::rewrite::{PathRegex, PathRewrite};

/// The path rewrite of a route.
#[derive(Debug, Default)]
pub struct Rewrite {
    /// The route path segments, when the route keeps its path.
    kept: Vec<String>,
    regex: Option<(PathRegex, String)>,
    prefix: Vec<String>,
}

impl Rewrite {
    /// `route_path` is the path of the route, e.g. `/api`.
    pub fn new(config: &PathRewrite, route_path: &str) -> Self {
        Self {
            kept: if config.strip_prefix {
                Vec::new()
            } else {
                path_segments(route_path)
            },
            regex: config
                .regex
                .clone()
                .map(|regex| (regex, config.replacement.clone())),
            prefix: path_segments(&config.add_prefix),
        }
    }

    /// Returns the segments that follow the server path, from the request path segments after the
    /// route path.
    pub fn apply(&self, rest: Vec<String>) -> Vec<String> {
        let mut segments = [self.kept.clone(), rest].concat();
        if let Some((regex, replacement)) = &self.regex {
            segments = split(&regex.replace(&join(&segments), replacement));
        }
        [self.prefix.clone(), segments].concat()
    }
}

/// The non-empty segments of a path.
fn path_segments(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// Writes `["a", "b"]` as `/a/b`, `[""]` as `/` and `[]` as the empty path.
fn join(segments: &[String]) -> String {
    segments
        .iter()
        .map(|segment| format!("/{segment}"))
        .collect()
}

/// Reads a path that [`join`] wrote, and a path without the leading `/`.
fn split(path: &str) -> Vec<String> {
    if path.is_empty() {
        return Vec::new();
    }
    path.strip_prefix('/')
        .unwrap_or(path)
        .split('/')
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn the_rewrite_steps_run_in_order() {
        let default = Rewrite::new(&PathRewrite::default(), "/api");
        assert_eq!(default.apply(segments(&["users"])), ["users"]);

        let kept = PathRewrite {
            strip_prefix: false,
            ..Default::default()
        };
        let kept = Rewrite::new(&kept, "/api");
        assert_eq!(kept.apply(segments(&["users"])), ["api", "users"]);

        let all = PathRewrite {
            strip_prefix: false,
            regex: Some("^/api/items/([0-9]+)$".parse().unwrap()),
            replacement: "/item/${1}".into(),
            add_prefix: "/v2/".into(),
        };
        let all = Rewrite::new(&all, "/api");
        assert_eq!(all.apply(segments(&["items", "42"])), ["v2", "item", "42"]);
        assert_eq!(all.apply(segments(&["other"])), ["v2", "api", "other"]);
    }

    #[test]
    fn the_empty_path_and_a_trailing_slash_survive_the_regex() {
        let rewrite = PathRewrite {
            regex: Some("^/x".parse().unwrap()),
            replacement: "/y".into(),
            ..Default::default()
        };
        let rewrite = Rewrite::new(&rewrite, "");
        assert_eq!(rewrite.apply(Vec::new()), Vec::<String>::new());
        assert_eq!(rewrite.apply(segments(&[""])), [""]);
        assert_eq!(rewrite.apply(segments(&["x", ""])), ["y", ""]);
    }
}
