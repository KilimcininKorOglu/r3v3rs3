//! General scheduled commands scheduler for platform apps. Runs commands inside the container
//! of a running deployment according to cron, time-of-day or interval schedules.

use super::{Platform, proxy};
use crate::clock::unix_ms;
use anyhow::Context as _;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppScheduledCommand, ScheduledCommandRequest, ScheduledCommandRunResult,
};
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;

/// How often the scheduler wakes up to check for due commands.
const SCHEDULER_INTERVAL: Duration = Duration::from_secs(30);

/// Execution timeout for a scheduled command (5 minutes).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

/// Maximum output bytes recorded for a command execution.
const MAX_OUTPUT_CHARS: usize = 4096;

/// Holds an executing scheduled command so that only one instance runs at a time.
pub struct ScheduledCommandLock {
    pub(super) platform: Arc<Platform>,
    pub(super) cmd_id: ShortId,
}

impl Drop for ScheduledCommandLock {
    fn drop(&mut self) {
        self.platform
            .running_scheduled_commands()
            .remove(&self.cmd_id);
    }
}

/// Spawns the background scheduler loop.
pub fn spawn_command_scheduler(platform: &Arc<Platform>) {
    let platform = Arc::downgrade(platform);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SCHEDULER_INTERVAL).await;
            let Some(platform) = platform.upgrade() else {
                break;
            };
            platform.run_due_commands().await;
            platform.check_window_deployments().await;
        }
    });
}

impl Platform {
    /// Checks pending deployments whose maintenance window has opened and approves them.
    pub async fn check_window_deployments(self: &Arc<Self>) {
        let Ok(pending) = self.store.pending_deployments().await else {
            return;
        };
        let now = OffsetDateTime::now_utc();
        let hour = now.hour() as u32;
        let minute = now.minute() as u32;
        for dep in pending {
            let Ok(app) = self.app(dep.app).await else {
                continue;
            };
            if app.spec.require_approval {
                continue;
            }
            let Some(window) = app.spec.deploy_window.as_deref() else {
                continue;
            };
            if r3v3rs3_api::platform::is_within_deploy_window(window, hour, minute) {
                let this = self.clone();
                let dep_id = dep.id;
                tokio::spawn(async move {
                    if let Err(err) = this.approve_deployment(dep_id, "scheduler").await {
                        tracing::warn!(err = format!("{err:#}"), %dep_id, "window deployment auto-approval failed");
                    }
                });
            }
        }
    }
    /// Checks all enabled scheduled commands and triggers those that are due.
    pub async fn run_due_commands(self: &Arc<Self>) {
        let Ok(commands) = self.store.list_all_enabled_scheduled_commands().await else {
            return;
        };
        let now = unix_ms() as i64;
        for cmd in commands {
            if is_schedule_due(&cmd.schedule, cmd.last_run_at, now) {
                let Some(lock) = self.lock_scheduled_command(cmd.id) else {
                    continue;
                };
                let this = self.clone();
                let cmd_id = cmd.id;
                tokio::spawn(async move {
                    let _lock = lock;
                    if let Err(err) = this.execute_scheduled_command_impl(cmd_id).await {
                        tracing::warn!(err = format!("{err:#}"), %cmd_id, "scheduled command failed");
                    }
                });
            }
        }
    }

    /// Executes a single scheduled command inside the container of its app.
    pub async fn execute_scheduled_command(
        self: &Arc<Self>,
        cmd_id: ShortId,
    ) -> anyhow::Result<ScheduledCommandRunResult> {
        let Some(_lock) = self.lock_scheduled_command(cmd_id) else {
            anyhow::bail!("scheduled command {cmd_id} is already running");
        };
        self.execute_scheduled_command_impl(cmd_id).await
    }

    async fn execute_scheduled_command_impl(
        &self,
        cmd_id: ShortId,
    ) -> anyhow::Result<ScheduledCommandRunResult> {
        let Some(cmd) = self.store.scheduled_command(cmd_id).await? else {
            anyhow::bail!("scheduled command {cmd_id} not found");
        };
        let app = self.app(cmd.app_id).await?;
        let deployment = self
            .store
            .running_deployment(cmd.app_id)
            .await?
            .context("the app has no running deployment")?;
        let container = proxy::container_name(cmd.app_id, deployment)?;
        let runtime = self.runtime(app.target);

        let start = std::time::Instant::now();
        let exec_cmd = vec!["sh".into(), "-c".into(), cmd.command.clone()];
        let run_at = unix_ms() as i64;

        let exec_future = runtime.exec_capture(&container, exec_cmd, Vec::new());
        let result = tokio::time::timeout(COMMAND_TIMEOUT, exec_future).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        let (success, status, output) = format_command_result(result);

        self.store
            .record_scheduled_command_run(cmd_id, run_at, &status, &output)
            .await?;

        Ok(ScheduledCommandRunResult {
            success,
            status,
            output,
            duration_ms,
        })
    }

    /// Lists all scheduled commands of an app.
    pub async fn scheduled_commands(
        &self,
        app_id: ShortId,
    ) -> anyhow::Result<Vec<AppScheduledCommand>> {
        self.app(app_id).await?;
        self.store.list_scheduled_commands(app_id).await
    }

    /// Adds a new scheduled command for an app.
    pub async fn add_scheduled_command(
        &self,
        app_id: ShortId,
        req: ScheduledCommandRequest,
    ) -> anyhow::Result<AppScheduledCommand> {
        self.app(app_id).await?;
        let now = unix_ms() as i64;
        let cmd = AppScheduledCommand {
            id: ShortId::new(),
            app_id,
            name: req.name.trim().to_string(),
            schedule: req.schedule.trim().to_string(),
            command: req.command.trim().to_string(),
            enabled: req.enabled,
            last_run_at: None,
            last_status: None,
            last_output: None,
            created_at: now,
            updated_at: now,
        };
        self.store.insert_scheduled_command(&cmd).await?;
        Ok(cmd)
    }

    /// Updates an existing scheduled command.
    pub async fn update_scheduled_command(
        &self,
        cmd_id: ShortId,
        req: ScheduledCommandRequest,
    ) -> anyhow::Result<AppScheduledCommand> {
        let Some(mut cmd) = self.store.scheduled_command(cmd_id).await? else {
            return Err(r3v3rs3_api::error::Error::ScheduledCommandNotFound { id: cmd_id }.into());
        };
        cmd.name = req.name.trim().to_string();
        cmd.schedule = req.schedule.trim().to_string();
        cmd.command = req.command.trim().to_string();
        cmd.enabled = req.enabled;
        cmd.updated_at = unix_ms() as i64;
        self.store.update_scheduled_command(&cmd).await?;
        Ok(cmd)
    }

    /// Deletes a scheduled command.
    pub async fn delete_scheduled_command(&self, cmd_id: ShortId) -> anyhow::Result<()> {
        let Some(_) = self.store.scheduled_command(cmd_id).await? else {
            return Err(r3v3rs3_api::error::Error::ScheduledCommandNotFound { id: cmd_id }.into());
        };
        self.store.delete_scheduled_command(cmd_id).await
    }
}

fn format_command_result(
    result: Result<Result<Vec<u8>, anyhow::Error>, tokio::time::error::Elapsed>,
) -> (bool, String, String) {
    match result {
        Ok(Ok(bytes)) => {
            let text = String::from_utf8_lossy(&bytes);
            let truncated = if text.len() > MAX_OUTPUT_CHARS {
                format!("{}... [truncated]", &text[..MAX_OUTPUT_CHARS])
            } else {
                text.into_owned()
            };
            (true, "ok".to_string(), truncated)
        }
        Ok(Err(err)) => {
            let err_msg = format!("{err:#}");
            (false, format!("failed: {err_msg}"), err_msg)
        }
        Err(_) => {
            let err_msg = format!("command timed out after {}s", COMMAND_TIMEOUT.as_secs());
            (false, format!("failed: {err_msg}"), err_msg)
        }
    }
}

/// Checks whether a given schedule expression is due at `now_ms` based on `last_run_at`.
pub fn is_schedule_due(schedule: &str, last_run_at: Option<i64>, now_ms: i64) -> bool {
    let trimmed = schedule.trim();
    if let Some(slot) = daily_slot_ms(trimmed, now_ms) {
        return now_ms >= slot && last_run_at.is_none_or(|last| last < slot);
    }
    if let Some(interval) = parse_interval_ms(trimmed) {
        return last_run_at.is_none_or(|last| now_ms.saturating_sub(last) >= interval);
    }
    let Ok(dt) = OffsetDateTime::from_unix_timestamp(now_ms / 1000) else {
        return false;
    };
    if match_cron_5(trimmed, &dt) {
        let current_minute = (now_ms / 60_000) * 60_000;
        return last_run_at.is_none_or(|last| last < current_minute);
    }
    false
}

/// Computes the UTC millisecond timestamp of today's `HH:MM` time.
fn daily_slot_ms(schedule: &str, now_ms: i64) -> Option<i64> {
    let (hour, minute) = schedule.split_once(':')?;
    let hour: i64 = hour.parse().ok()?;
    let minute: i64 = minute.parse().ok()?;
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    let day = now_ms.div_euclid(86_400_000);
    Some(day * 86_400_000 + hour * 3_600_000 + minute * 60_000)
}

/// Parses intervals such as `@hourly`, `@daily`, `30m`, `1h`, `@every 15m`.
fn parse_interval_ms(expr: &str) -> Option<i64> {
    let clean = expr.trim().strip_prefix("@every ").unwrap_or(expr).trim();
    match clean {
        "@hourly" | "hourly" => return Some(3_600_000),
        "@daily" | "daily" => return Some(86_400_000),
        _ => {}
    }
    if let Some(num) = clean.strip_suffix('m') {
        let m: i64 = num.parse().ok()?;
        return Some(m * 60_000);
    }
    if let Some(num) = clean.strip_suffix('h') {
        let h: i64 = num.parse().ok()?;
        return Some(h * 3_600_000);
    }
    if let Some(num) = clean.strip_suffix('s') {
        let s: i64 = num.parse().ok()?;
        return Some(s * 1_000);
    }
    None
}

/// Evaluates a 5-field cron expression: `minute hour dom month dow`.
fn match_cron_5(schedule: &str, dt: &OffsetDateTime) -> bool {
    let fields: Vec<&str> = schedule.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }
    let min = dt.minute() as u32;
    let hour = dt.hour() as u32;
    let dom = dt.day() as u32;
    let month = dt.month() as u32;
    let dow = dt.weekday().number_days_from_sunday() as u32;

    match_cron_field(fields[0], min)
        && match_cron_field(fields[1], hour)
        && match_cron_field(fields[2], dom)
        && match_cron_field(fields[3], month)
        && match_cron_field(fields[4], dow)
}

/// Matches a single cron field against an integer value.
fn match_cron_field(expr: &str, val: u32) -> bool {
    if expr == "*" {
        return true;
    }
    if let Some(step_str) = expr.strip_prefix("*/")
        && let Ok(step) = step_str.parse::<u32>()
    {
        return step > 0 && val.is_multiple_of(step);
    }
    for part in expr.split(',') {
        if let Ok(n) = part.parse::<u32>()
            && n == val
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_schedule_due_check() {
        let schedule = "04:30";
        let slot = daily_slot_ms(schedule, 100_000_000).unwrap();
        assert!(!is_schedule_due(schedule, None, slot - 1000));
        assert!(is_schedule_due(schedule, None, slot + 1000));
        assert!(!is_schedule_due(schedule, Some(slot + 500), slot + 1000));
    }

    #[test]
    fn interval_schedule_due_check() {
        let now = 1_000_000_000;
        assert!(is_schedule_due("15m", None, now));
        assert!(is_schedule_due("15m", Some(now - 15 * 60_000), now));
        assert!(!is_schedule_due("15m", Some(now - 10 * 60_000), now));
        assert!(is_schedule_due("@hourly", Some(now - 3_600_000), now));
    }

    #[test]
    fn cron_5_field_matching() {
        let dt = OffsetDateTime::from_unix_timestamp(1700000000).unwrap();
        assert!(match_cron_field("*", 5));
        assert!(match_cron_field("*/5", 15));
        assert!(!match_cron_field("*/5", 16));
        assert!(match_cron_field("1,2,5", 5));
        assert!(!match_cron_field("1,2,5", 3));
        assert!(match_cron_5("* * * * *", &dt));
    }

    #[test]
    fn format_command_result_success_and_errors() {
        let ok_res = Ok(Ok(b"hello world".to_vec()));
        let (success, status, output) = format_command_result(ok_res);
        assert!(success);
        assert_eq!(status, "ok");
        assert_eq!(output, "hello world");

        let err_res = Ok(Err(anyhow::anyhow!("exec failed")));
        let (success, status, output) = format_command_result(err_res);
        assert!(!success);
        assert!(status.contains("failed: exec failed"));
        assert_eq!(output, "exec failed");
    }
}
