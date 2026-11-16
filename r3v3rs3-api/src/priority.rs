//! HTTP/2 and HTTP/3 Adaptive Stream Priority (RFC 9218 - Extensible Prioritization Scheme).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use utoipa::ToSchema;

/// Default limit for the concurrent low-priority background streams of one client when adaptive
/// concurrency is enabled.
pub const DEFAULT_MAX_BACKGROUND_CONCURRENCY: usize = 32;

/// Global singleton tracking stream priority metrics and adaptive backpressure.
pub static GLOBAL_STREAM_CONTROLLER: LazyLock<Arc<AdaptiveStreamController>> =
    LazyLock::new(|| Arc::new(AdaptiveStreamController::new()));

/// RFC 9218 Stream Priority parameters (`Priority: u=urgency, i=?incremental`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema,
)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema)]
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

    /// RFC 9218 section 4.1: an urgency lies from 0 to 7, so a rule outside it is refused
    /// instead of being moved to the nearest level.
    pub fn validate(&self) -> Result<(), crate::error::Error> {
        match self.custom_rules.iter().find(|rule| rule.urgency > 7) {
            Some(rule) => Err(crate::error::Error::InvalidPriorityUrgency {
                urgency: rule.urgency,
            }),
            None => Ok(()),
        }
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

/// RFC 9218 section 4: a parameter with an out-of-range value or a value of an unexpected type
/// is ignored. `u` is an Integer from 0 to 7 and `i` a Boolean (`?1`, `?0` or a bare `i`);
/// RFC 8941 section 3.2 allows no whitespace around `=`.
fn parse_rfc9218_member(member: &str) -> Option<MemberParsed> {
    match member.split_once('=') {
        Some(("u", value)) => parse_urgency(value).map(MemberParsed::Urgency),
        Some(("i", "?1")) => Some(MemberParsed::Incremental(true)),
        Some(("i", "?0")) => Some(MemberParsed::Incremental(false)),
        None if member == "i" => Some(MemberParsed::Incremental(true)),
        _ => None,
    }
}

/// Parses an RFC 8941 Integer of digits only and keeps it when it lies in 0..=7.
fn parse_urgency(value: &str) -> Option<u8> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<u8>().ok().filter(|urgency| *urgency <= 7)
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
#[derive(Debug, Default, Clone, Serialize, Deserialize, ToSchema, schemars::JsonSchema)]
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
    /// Active background streams per client, so one client cannot use up the limit of all.
    /// An entry is removed when its count reaches 0.
    background_per_client: Mutex<HashMap<IpAddr, usize>>,
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

    /// Acquires a permit for a stream of `client`. A background stream is refused (`None`) when
    /// `client` already holds `max_background` background streams; `None` as the limit means no
    /// limit. Other streams always get a permit.
    pub fn try_acquire(
        self: &Arc<Self>,
        priority: StreamPriority,
        client: IpAddr,
        max_background: Option<usize>,
    ) -> Option<StreamPermit> {
        let background_client = if priority.is_background() {
            let client = client.to_canonical();
            if !self.take_background_slot(client, max_background) {
                self.throttled_background.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            Some(client)
        } else {
            None
        };
        self.active_streams.fetch_add(1, Ordering::Relaxed);
        Some(StreamPermit {
            controller: self.clone(),
            background_client,
        })
    }

    /// Counts one more background stream of `client` unless it reached the limit.
    fn take_background_slot(&self, client: IpAddr, max_background: Option<usize>) -> bool {
        let mut counts = self
            .background_per_client
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let count = counts.get(&client).copied().unwrap_or(0);
        if max_background.is_some_and(|max| count >= max) {
            return false;
        }
        counts.insert(client, count + 1);
        true
    }

    /// Releases one background stream of `client`.
    fn release_background_slot(&self, client: IpAddr) {
        let mut counts = self
            .background_per_client
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = counts.get_mut(&client) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(&client);
            }
        }
    }

    /// The number of clients that hold a background stream.
    pub fn background_clients(&self) -> usize {
        self.background_per_client
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
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
    background_client: Option<IpAddr>,
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        self.controller
            .active_streams
            .fetch_sub(1, Ordering::Relaxed);
        if let Some(client) = self.background_client {
            self.controller.release_background_slot(client);
        }
    }
}

/// REST API request payload for updating stream priority configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema)]
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

    /// RFC 9218 section 4: out-of-range values and values of an unexpected type are ignored,
    /// so the other members of the field still count and the ignored one keeps its default.
    #[test]
    fn invalid_priority_members_are_ignored() {
        for header in [
            "u=8", "u=255", "u=-1", "u=+2", "u=", "u=1.5", "u =2", "i=1", "i=0",
        ] {
            assert_eq!(parse_rfc9218_priority(header), None, "{header}");
        }
        assert_eq!(
            parse_rfc9218_priority("u=9, i"),
            Some(StreamPriority::new(3, true))
        );
        assert_eq!(
            parse_rfc9218_priority("u=1, i=1"),
            Some(StreamPriority::new(1, false))
        );
    }

    #[test]
    fn test_header_value_formatting() {
        assert_eq!(StreamPriority::new(1, false).to_header_value(), "u=1");
        assert_eq!(StreamPriority::new(5, true).to_header_value(), "u=5, i");
    }

    /// RFC 9218 section 4.1: a lower urgency is a higher priority, so it weighs more.
    #[test]
    fn test_weight_mapping() {
        for urgency in 0..7 {
            let higher = StreamPriority::new(urgency, false).weight();
            let lower = StreamPriority::new(urgency + 1, true).weight();
            assert!(higher > lower, "u={urgency}: {higher} <= {lower}");
        }
    }

    /// The document comes before the styles and the API calls, which come before the images and
    /// the videos (RFC 9218 section 4.1); images and videos render progressively, so they are
    /// incremental (section 4.2).
    #[test]
    fn test_heuristic_resolution() {
        let config = StreamPriorityConfig {
            enabled: true,
            ..Default::default()
        };
        let resolve = |path| resolve_stream_priority(None, path, None, &config);
        let html = resolve("/index.html");
        let css = resolve("/style.css");
        let api = resolve("/api/v1/users");
        let image = resolve("/banner.webp");
        let video = resolve("/video.mp4");

        assert!(html.urgency < css.urgency, "{html:?} {css:?}");
        assert!(
            css.urgency.max(api.urgency) < image.urgency,
            "{css:?} {api:?} {image:?}"
        );
        assert!(image.urgency <= video.urgency, "{image:?} {video:?}");
        assert!(!html.incremental && image.incremental && video.incremental);
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

        let client = IpAddr::from([192, 0, 2, 1]);
        let permit1 = controller.try_acquire(crit, client, Some(1)).unwrap();
        let permit2 = controller.try_acquire(bg, client, Some(1)).unwrap();

        let stats = controller.stats();
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.critical_requests, 1);
        assert_eq!(stats.background_requests, 1);
        assert_eq!(stats.active_streams, 2);

        drop(permit1);
        drop(permit2);

        assert_eq!(controller.stats().active_streams, 0);
    }

    /// RFC 9218 section 4.1: urgency 7 is the lowest level, so a rule with 8 is refused.
    #[test]
    fn a_rule_outside_the_urgency_range_is_refused() {
        let rule = |urgency| MimePriorityRule {
            pattern: "/api/*".into(),
            urgency,
            incremental: false,
        };
        let config = |urgency| StreamPriorityConfig {
            enabled: true,
            custom_rules: vec![rule(0), rule(urgency)],
            ..Default::default()
        };
        assert!(config(7).validate().is_ok());
        for urgency in [8, 255] {
            assert!(matches!(
                config(urgency).validate(),
                Err(crate::error::Error::InvalidPriorityUrgency { urgency: u }) if u == urgency
            ));
        }
    }

    /// One client that holds its background streams must not block the background streams of
    /// another client, and a released client leaves no entry behind.
    #[test]
    fn background_limit_counts_each_client() {
        let controller = Arc::new(AdaptiveStreamController::new());
        let bg = StreamPriority::new(7, false);
        let attacker = IpAddr::from([192, 0, 2, 1]);
        let other = IpAddr::from([198, 51, 100, 7]);

        let held: Vec<_> = (0..2)
            .map(|_| controller.try_acquire(bg, attacker, Some(2)).unwrap())
            .collect();
        assert!(controller.try_acquire(bg, attacker, Some(2)).is_none());
        assert_eq!(controller.stats().throttled_background, 1);
        let other_permit = controller.try_acquire(bg, other, Some(2));
        assert!(other_permit.is_some());
        // An IPv4-mapped IPv6 address is the same client.
        let mapped = IpAddr::from(std::net::Ipv4Addr::new(192, 0, 2, 1).to_ipv6_mapped());
        assert!(controller.try_acquire(bg, mapped, Some(2)).is_none());

        drop(held);
        drop(other_permit);
        assert_eq!(controller.background_clients(), 0);
        assert!(controller.try_acquire(bg, attacker, Some(2)).is_some());
        assert!(controller.try_acquire(bg, attacker, None).is_some());
    }
}
