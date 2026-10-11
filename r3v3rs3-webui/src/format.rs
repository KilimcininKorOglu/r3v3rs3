use r3v3rs3_api::i18n::Locale;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use wasm_bindgen::UnwrapThrowExt;
use web_time::{SystemTime, UNIX_EPOCH};

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

/// Formats a Unix time as its date and the exact days left until it,
/// e.g. `2026-12-01 (59 days)`. A time less than a day away names its hours.
pub fn format_duration(locale: Locale, unix_time: i64) -> String {
    let time = OffsetDateTime::from_unix_timestamp(unix_time).unwrap_throw();
    let timestamp = time.format(&Rfc3339).unwrap_throw();
    let date = timestamp.split('T').next().unwrap_throw();
    let remaining = u64::try_from(unix_time)
        .ok()
        .zip(u64::try_from(unix_now()).ok())
        .and_then(|(time, now)| time.checked_sub(now));
    format!("{date} ({})", time_left(locale, remaining))
}

/// A Unix time in milliseconds in the RFC 3339 form, in UTC.
pub fn format_time(ms: u64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
        .unwrap_or_else(|| ms.to_string())
}

/// The current Unix time in seconds.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Names the remaining time in exact days, or in hours when less than a day is left.
fn time_left(locale: Locale, seconds: Option<u64>) -> String {
    let Some(seconds) = seconds.filter(|seconds| *seconds > 0) else {
        return locale.t("time.expired").to_string();
    };
    if seconds >= DAY {
        return locale.tf("time.days", &[("count", &(seconds / DAY).to_string())]);
    }
    if seconds >= HOUR {
        return locale.tf("time.hours", &[("count", &(seconds / HOUR).to_string())]);
    }
    locale.t("time.less_than_hour").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_names_the_exact_days_and_hours() {
        assert_eq!(time_left(Locale::En, Some(400 * DAY)), "400 days");
        assert_eq!(time_left(Locale::En, Some(59 * DAY + 12 * HOUR)), "59 days");
        assert_eq!(time_left(Locale::Tr, Some(2 * DAY)), "2 gün");
        assert_eq!(time_left(Locale::Tr, Some(2 * HOUR + 5)), "2 saat");
        assert_eq!(time_left(Locale::Tr, Some(60)), "bir saatten az");
        assert_eq!(time_left(Locale::En, Some(0)), "Expired");
        assert_eq!(time_left(Locale::Tr, None), "Süresi doldu");
    }
}
