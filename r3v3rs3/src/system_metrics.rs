//! The system and process self-monitoring metrics: open file descriptors, memory usage, and event loop lag.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static EVENT_LOOP_LAG_MICROS: AtomicU64 = AtomicU64::new(0);
static LAG_MONITOR_STARTED: OnceLock<()> = OnceLock::new();

/// Starts the background lag monitor task if not already started.
pub fn ensure_lag_monitor_started() {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        LAG_MONITOR_STARTED.get_or_init(|| {
            handle.spawn(async {
                let interval = Duration::from_millis(100);
                loop {
                    let start = Instant::now();
                    tokio::time::sleep(interval).await;
                    let elapsed = start.elapsed();
                    let lag = elapsed.saturating_sub(interval);
                    EVENT_LOOP_LAG_MICROS.store(lag.as_micros() as u64, Ordering::Relaxed);
                }
            });
        });
    }
}

/// The current event loop lag in microseconds.
pub fn event_loop_lag_micros() -> u64 {
    ensure_lag_monitor_started();
    EVENT_LOOP_LAG_MICROS.load(Ordering::Relaxed)
}

/// The number of open file descriptors in this process, read from `/proc/self/fd`.
pub fn open_fds() -> Option<u64> {
    std::fs::read_dir("/proc/self/fd")
        .ok()
        .map(|entries| entries.count() as u64)
}

/// The maximum number of file descriptors this process can open, read from `rlimit`.
pub fn max_fds() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 {
        Some(limit.rlim_cur)
    } else {
        None
    }
}

/// The resident and virtual memory size in bytes, read from `/proc/self/statm`.
pub fn memory_bytes() -> (Option<u64>, Option<u64>) {
    let Ok(content) = std::fs::read_to_string("/proc/self/statm") else {
        return (None, None);
    };
    let mut parts = content.split_whitespace();
    let vm_pages = parts.next().and_then(|p| p.parse::<u64>().ok());
    let rss_pages = parts.next().and_then(|p| p.parse::<u64>().ok());
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = if page_size > 0 {
        page_size as u64
    } else {
        4096
    };
    (
        rss_pages.map(|p| p * page_size),
        vm_pages.map(|p| p * page_size),
    )
}

/// Host socket statistics parsed from `/proc/net/sockstat`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct SocketStats {
    pub total_used: Option<u64>,
    pub tcp_inuse: Option<u64>,
    pub tcp_tw: Option<u64>,
}

/// Reads and parses `/proc/net/sockstat`.
pub fn socket_stats() -> SocketStats {
    let Ok(content) = std::fs::read_to_string("/proc/net/sockstat") else {
        return SocketStats::default();
    };
    parse_sockstat(&content)
}

fn parse_sockstat(content: &str) -> SocketStats {
    let mut stats = SocketStats::default();
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        if parts[0] == "sockets:" && parts.len() >= 3 && parts[1] == "used" {
            stats.total_used = parts[2].parse::<u64>().ok();
        } else if parts[0] == "TCP:" {
            let mut i = 1;
            while i + 1 < parts.len() {
                if parts[i] == "inuse" {
                    stats.tcp_inuse = parts[i + 1].parse::<u64>().ok();
                } else if parts[i] == "tw" {
                    stats.tcp_tw = parts[i + 1].parse::<u64>().ok();
                }
                i += 2;
            }
        }
    }
    stats
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemMetricsSnapshot {
    pub open_fds: Option<u64>,
    pub max_fds: Option<u64>,
    pub fd_usage_percent: Option<f64>,
    pub resident_memory_bytes: Option<u64>,
    pub virtual_memory_bytes: Option<u64>,
    pub event_loop_lag_seconds: f64,
    pub sockets_used: Option<u64>,
    pub tcp_inuse: Option<u64>,
    pub tcp_tw: Option<u64>,
}

pub fn collect_system_metrics() -> SystemMetricsSnapshot {
    let (rss, vm) = memory_bytes();
    let open = open_fds();
    let max = max_fds();
    let fd_usage_percent = match (open, max) {
        (Some(o), Some(m)) if m > 0 => Some(((o as f64 / m as f64) * 1000.0).round() / 10.0),
        _ => None,
    };
    let sock = socket_stats();
    SystemMetricsSnapshot {
        open_fds: open,
        max_fds: max,
        fd_usage_percent,
        resident_memory_bytes: rss,
        virtual_memory_bytes: vm,
        event_loop_lag_seconds: event_loop_lag_micros() as f64 / 1_000_000.0,
        sockets_used: sock.total_used,
        tcp_inuse: sock.tcp_inuse,
        tcp_tw: sock.tcp_tw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn system_metrics_returns_valid_snapshot() {
        let metrics = collect_system_metrics();
        assert!(metrics.event_loop_lag_seconds >= 0.0);
    }

    #[test]
    fn parse_sockstat_sample() {
        let sample =
            "sockets: used 342\nTCP: inuse 28 orphan 0 tw 142 alloc 35 mem 3\nUDP: inuse 7 mem 3\n";
        let stats = parse_sockstat(sample);
        assert_eq!(stats.total_used, Some(342));
        assert_eq!(stats.tcp_inuse, Some(28));
        assert_eq!(stats.tcp_tw, Some(142));
    }
}
