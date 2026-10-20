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

#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemMetricsSnapshot {
    pub open_fds: Option<u64>,
    pub max_fds: Option<u64>,
    pub resident_memory_bytes: Option<u64>,
    pub virtual_memory_bytes: Option<u64>,
    pub event_loop_lag_seconds: f64,
}

pub fn collect_system_metrics() -> SystemMetricsSnapshot {
    let (rss, vm) = memory_bytes();
    SystemMetricsSnapshot {
        open_fds: open_fds(),
        max_fds: max_fds(),
        resident_memory_bytes: rss,
        virtual_memory_bytes: vm,
        event_loop_lag_seconds: event_loop_lag_micros() as f64 / 1_000_000.0,
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
}
