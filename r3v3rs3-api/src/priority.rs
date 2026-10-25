//! HTTP/2 and HTTP/3 Adaptive Stream Priority (RFC 9218 - Extensible Prioritization Scheme).

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use utoipa::ToSchema;

/// Global singleton tracking stream priority metrics and adaptive backpressure.
pub static GLOBAL_STREAM_CONTROLLER: LazyLock<AdaptiveStreamController> =
    LazyLock::new(AdaptiveStreamController::new);

/// RFC 9218 Stream Priority parameters (`Priority: u=urgency, i=?incremental`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StreamPriority {
    /// Urgency level from 0 (highest/critical) to 7 (lowest/background). Default is 3.
    pub urgency: u8,
    /// Incremental streaming flag. When true, chunks can be delivered concurrently.
    pub incremental: bool,
}

impl Default for StreamPriority {
    fn default() -> Self {
        Self {
            urgency: 3,
            incremental: false,
        }
    }
}

impl StreamPriority {
    /// Creates a new `StreamPriority`, clamping urgency to 0..=7.
    pub fn new(urgency: u8, incremental: bool) -> Self {
        Self {
            urgency: urgency.min(7),
            incremental,
        }
    }

    /// Formats this stream priority according to RFC 9218 Structured Fields dictionary format.
    pub fn to_header_value(&self) -> String {
        if self.incremental {
            format!("u={}, i", self.urgency)
        } else {
            format!("u={}", self.urgency)
        }
    }

    /// Maps RFC 9218 urgency to scheduling weight (0: 256, 1: 128, ..., 7: 2).
    pub fn weight(&self) -> u32 {
        match self.urgency {
            0 => 256,
            1 => 128,
            2 => 64,
            3 => 32,
            4 => 16,
            5 => 8,
            6 => 4,
            7 => 2,
            _ => 32,
        }
    }

    /// Returns true if this stream is on the critical rendering / application path (u <= 2).
    pub fn is_critical(&self) -> bool {
        self.urgency <= 2
    }

    /// Returns true if this stream is a background or low-priority task (u >= 5).
    pub fn is_background(&self) -> bool {
        self.urgency >= 5
    }
}

/// Custom rule overriding stream priority based on MIME or path pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MimePriorityRule {
    /// Path or MIME pattern (e.g. `*.css`, `image/*`, `/api/critical/*`).
    pub pattern: String,
    /// Urgency level (0..7).
    pub urgency: u8,
    /// Incremental streaming flag.
    #[serde(default)]
    pub incremental: bool,
}

impl MimePriorityRule {
    /// Checks whether this rule matches the request path or content type.
    pub fn matches(&self, path: &str, content_type: Option<&str>) -> bool {
        let pattern = self.pattern.trim();
        if pattern.is_empty() {
            return false;
        }
        if let Some(ct) = content_type
            && matches_content_type(pattern, ct)
        {
            return true;
        }
        matches_path(pattern, path)
    }
}

fn matches_content_type(pattern: &str, ct: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/*") {
        ct.starts_with(prefix)
    } else {
        ct.eq_ignore_ascii_case(pattern)
    }
}

fn matches_path(pattern: &str, path: &str) -> bool {
    if let Some(ext) = pattern.strip_prefix("*.") {
        path.ends_with(ext)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        path.starts_with(prefix)
    } else {
        path == pattern
    }
}

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// Configuration for HTTP/2 & HTTP/3 adaptive stream prioritization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StreamPriorityConfig {
    /// When true, enables adaptive stream priority optimization.
    #[serde(default, skip_serializing_if = "is_false")]
    pub enabled: bool,
    /// When true, injects RFC 9218 `Priority: u=..., i` into upstream requests.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub inject_header: bool,
    /// When true, applies adaptive concurrency backpressure to low-priority background streams under load.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub adaptive_concurrency: bool,
    /// Custom path or MIME pattern priority overrides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_rules: Vec<MimePriorityRule>,
}

impl Default for StreamPriorityConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            inject_header: true,
            adaptive_concurrency: true,
            custom_rules: Vec::new(),
        }
    }
}

impl StreamPriorityConfig {
    /// Returns true if stream priority optimization is disabled.
    pub fn is_disabled(&self) -> bool {
        !self.enabled
    }
}

/// Parses an RFC 9218 `Priority` header string (`u=0..7`, `i`, `i=?1`, `i=?0`).
pub fn parse_rfc9218_priority(header_val: &str) -> Option<StreamPriority> {
    let s = header_val.trim();
    if s.is_empty() {
        return None;
    }

    let mut urgency = None;
    let mut incremental = false;
    let mut parsed_any = false;

    for part in s.split(',') {
        if let Some(res) = parse_rfc9218_member(part.trim()) {
            parsed_any = true;
            match res {
                MemberParsed::Urgency(u) => urgency = Some(u),
                MemberParsed::Incremental(inc) => incremental = inc,
            }
        }
    }

    if parsed_any {
        Some(StreamPriority {
            urgency: urgency.unwrap_or(3),
            incremental,
        })
    } else {
        None
    }
}

enum MemberParsed {
    Urgency(u8),
    Incremental(bool),
}

fn parse_rfc9218_member(member: &str) -> Option<MemberParsed> {
    if let Some(u_str) = member
        .strip_prefix("u=")
        .or_else(|| member.strip_prefix("u ="))
    {
        u_str
            .trim()
            .parse::<u8>()
            .ok()
            .map(|u| MemberParsed::Urgency(u.min(7)))
    } else if member == "i" || member == "i=?1" || member == "i=1" {
        Some(MemberParsed::Incremental(true))
    } else if member == "i=?0" || member == "i=0" {
        Some(MemberParsed::Incremental(false))
    } else {
        None
    }
}

/// Resolves stream priority by checking client header, custom rules, or MIME heuristics.
pub fn resolve_stream_priority(
    client_header: Option<&str>,
    path_and_query: &str,
    content_type: Option<&str>,
    config: &StreamPriorityConfig,
) -> StreamPriority {
    if let Some(hdr) = client_header
        && let Some(parsed) = parse_rfc9218_priority(hdr)
    {
        return parsed;
    }

    let path = path_and_query.split('?').next().unwrap_or(path_and_query);

    for rule in &config.custom_rules {
        if rule.matches(path, content_type) {
            return StreamPriority::new(rule.urgency, rule.incremental);
        }
    }

    heuristic_stream_priority(path, content_type)
}

/// Built-in heuristic priority based on file extension, path, and MIME type.
fn heuristic_stream_priority(path: &str, content_type: Option<&str>) -> StreamPriority {
    if let Some(prio) = content_type.and_then(heuristic_by_content_type) {
        return prio;
    }
    heuristic_by_path(path)
}

fn heuristic_by_content_type(ct: &str) -> Option<StreamPriority> {
    if ct.starts_with("text/html") {
        Some(StreamPriority::new(0, false))
    } else if is_script_or_api_ct(ct) {
        Some(StreamPriority::new(1, false))
    } else if ct.starts_with("font/") {
        Some(StreamPriority::new(2, false))
    } else if ct.starts_with("image/") {
        Some(StreamPriority::new(5, true))
    } else if ct.starts_with("video/") || ct.starts_with("audio/") {
        Some(StreamPriority::new(6, true))
    } else {
        None
    }
}

fn is_script_or_api_ct(ct: &str) -> bool {
    ct.starts_with("text/css")
        || ct.contains("javascript")
        || ct.starts_with("application/json")
        || ct.starts_with("application/grpc")
}

fn heuristic_by_path(path: &str) -> StreamPriority {
    let lower = path.to_ascii_lowercase();
    if is_html_path(&lower) {
        StreamPriority::new(0, false)
    } else if is_critical_script_or_api(&lower) {
        StreamPriority::new(1, false)
    } else if is_font_path(&lower) {
        StreamPriority::new(2, false)
    } else if is_icon_path(&lower) {
        StreamPriority::new(4, false)
    } else if is_media_path(&lower) {
        heuristic_media_path(&lower)
    } else {
        StreamPriority::default()
    }
}

fn is_html_path(p: &str) -> bool {
    p.ends_with(".html") || p.ends_with(".htm") || p == "/"
}

fn is_critical_script_or_api(p: &str) -> bool {
    p.ends_with(".css") || p.ends_with(".js") || p.ends_with(".mjs") || p.starts_with("/api/")
}

fn is_font_path(p: &str) -> bool {
    p.ends_with(".woff2") || p.ends_with(".woff") || p.ends_with(".ttf")
}

fn is_icon_path(p: &str) -> bool {
    p.ends_with(".svg") || p.ends_with(".ico")
}

fn is_media_path(p: &str) -> bool {
    is_image_ext(p) || is_video_ext(p) || is_archive_ext(p)
}

fn heuristic_media_path(p: &str) -> StreamPriority {
    if is_image_ext(p) {
        StreamPriority::new(5, true)
    } else if is_video_ext(p) {
        StreamPriority::new(6, true)
    } else if is_archive_ext(p) {
        StreamPriority::new(7, true)
    } else {
        StreamPriority::default()
    }
}

fn is_image_ext(p: &str) -> bool {
    p.ends_with(".png")
        || p.ends_with(".jpg")
        || p.ends_with(".jpeg")
        || p.ends_with(".webp")
        || p.ends_with(".avif")
        || p.ends_with(".gif")
}

fn is_video_ext(p: &str) -> bool {
    p.ends_with(".mp4") || p.ends_with(".webm") || p.ends_with(".mp3")
}

fn is_archive_ext(p: &str) -> bool {
    p.ends_with(".zip") || p.ends_with(".tar.gz") || p.ends_with(".iso")
}

/// Statistics on prioritized HTTP streams.
#[derive(Debug, Default, Clone, Serialize, Deserialize, ToSchema)]
pub struct StreamPriorityStats {
    pub total_requests: u64,
    pub critical_requests: u64,
    pub normal_requests: u64,
    pub background_requests: u64,
    pub throttled_background: u64,
    pub active_streams: usize,
}

/// Controller tracking in-flight streams and managing adaptive backpressure.
#[derive(Debug, Default)]
pub struct AdaptiveStreamController {
    total_requests: AtomicU64,
    critical_requests: AtomicU64,
    normal_requests: AtomicU64,
    background_requests: AtomicU64,
    throttled_background: AtomicU64,
    active_streams: AtomicUsize,
    active_background_streams: AtomicUsize,
}

impl AdaptiveStreamController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an incoming request and updates counters.
    pub fn record_request(&self, priority: &StreamPriority) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        if priority.is_critical() {
            self.critical_requests.fetch_add(1, Ordering::Relaxed);
        } else if priority.is_background() {
            self.background_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.normal_requests.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Checks if a background stream should experience adaptive backpressure.
    pub fn should_throttle_background(&self, max_background_concurrency: usize) -> bool {
        let active_bg = self.active_background_streams.load(Ordering::Relaxed);
        if active_bg >= max_background_concurrency {
            self.throttled_background.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Acquires a stream permit tracking active multiplexed streams.
    pub fn acquire_permit(self: &Arc<Self>, priority: StreamPriority) -> StreamPermit {
        self.active_streams.fetch_add(1, Ordering::Relaxed);
        if priority.is_background() {
            self.active_background_streams
                .fetch_add(1, Ordering::Relaxed);
        }
        StreamPermit {
            controller: self.clone(),
            is_background: priority.is_background(),
        }
    }

    /// Returns a snapshot of priority statistics.
    pub fn stats(&self) -> StreamPriorityStats {
        StreamPriorityStats {
            total_requests: self.total_requests.load(Ordering::Relaxed),
            critical_requests: self.critical_requests.load(Ordering::Relaxed),
            normal_requests: self.normal_requests.load(Ordering::Relaxed),
            background_requests: self.background_requests.load(Ordering::Relaxed),
            throttled_background: self.throttled_background.load(Ordering::Relaxed),
            active_streams: self.active_streams.load(Ordering::Relaxed),
        }
    }
}

/// RAII guard releasing active stream counts on drop.
pub struct StreamPermit {
    controller: Arc<AdaptiveStreamController>,
    is_background: bool,
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        self.controller
            .active_streams
            .fetch_sub(1, Ordering::Relaxed);
        if self.is_background {
            self.controller
                .active_background_streams
                .fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// REST API request payload for updating stream priority configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SetStreamPriorityRequest {
    pub config: StreamPriorityConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rfc9218_parsing() {
        assert_eq!(
            parse_rfc9218_priority("u=0"),
            Some(StreamPriority {
                urgency: 0,
                incremental: false
            })
        );
        assert_eq!(
            parse_rfc9218_priority("u=5, i"),
            Some(StreamPriority {
                urgency: 5,
                incremental: true
            })
        );
        assert_eq!(
            parse_rfc9218_priority("i=?1, u=2"),
            Some(StreamPriority {
                urgency: 2,
                incremental: true
            })
        );
        assert_eq!(
            parse_rfc9218_priority("u=6, i=?0"),
            Some(StreamPriority {
                urgency: 6,
                incremental: false
            })
        );
        assert_eq!(
            parse_rfc9218_priority("i"),
            Some(StreamPriority {
                urgency: 3,
                incremental: true
            })
        );
        assert_eq!(parse_rfc9218_priority(""), None);
        assert_eq!(parse_rfc9218_priority("invalid"), None);
    }

    #[test]
    fn test_header_value_formatting() {
        assert_eq!(StreamPriority::new(1, false).to_header_value(), "u=1");
        assert_eq!(StreamPriority::new(5, true).to_header_value(), "u=5, i");
    }

    #[test]
    fn test_weight_mapping() {
        assert_eq!(StreamPriority::new(0, false).weight(), 256);
        assert_eq!(StreamPriority::new(1, false).weight(), 128);
        assert_eq!(StreamPriority::new(3, false).weight(), 32);
        assert_eq!(StreamPriority::new(7, true).weight(), 2);
    }

    #[test]
    fn test_heuristic_resolution() {
        let config = StreamPriorityConfig {
            enabled: true,
            ..Default::default()
        };

        let html = resolve_stream_priority(None, "/index.html", None, &config);
        assert_eq!(html.urgency, 0);
        assert!(!html.incremental);

        let css = resolve_stream_priority(None, "/style.css", None, &config);
        assert_eq!(css.urgency, 1);

        let image = resolve_stream_priority(None, "/banner.webp", None, &config);
        assert_eq!(image.urgency, 5);
        assert!(image.incremental);

        let video = resolve_stream_priority(None, "/video.mp4", None, &config);
        assert_eq!(video.urgency, 6);
        assert!(video.incremental);

        let api = resolve_stream_priority(None, "/api/v1/users", None, &config);
        assert_eq!(api.urgency, 1);
    }

    #[test]
    fn test_custom_rule_resolution() {
        let mut config = StreamPriorityConfig {
            enabled: true,
            ..Default::default()
        };
        config.custom_rules.push(MimePriorityRule {
            pattern: "/api/critical/*".into(),
            urgency: 0,
            incremental: false,
        });

        let prio = resolve_stream_priority(None, "/api/critical/order", None, &config);
        assert_eq!(prio.urgency, 0);
    }

    #[test]
    fn test_controller_stats_and_permits() {
        let controller = Arc::new(AdaptiveStreamController::new());
        let crit = StreamPriority::new(1, false);
        let bg = StreamPriority::new(6, true);

        controller.record_request(&crit);
        controller.record_request(&bg);

        let permit1 = controller.acquire_permit(crit);
        let permit2 = controller.acquire_permit(bg);

        let stats = controller.stats();
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.critical_requests, 1);
        assert_eq!(stats.background_requests, 1);
        assert_eq!(stats.active_streams, 2);

        drop(permit1);
        drop(permit2);

        assert_eq!(controller.stats().active_streams, 0);
    }
}
