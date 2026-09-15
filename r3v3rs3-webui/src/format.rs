use r3v3rs3_api::i18n::Locale;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use wasm_bindgen::UnwrapThrowExt;
use web_time::{SystemTime, UNIX_EPOCH};

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

/// Units of the remaining time, largest first, with the keys of one and of several units.
const UNITS: [(u64, &str, &str); 5] = [
    (365 * DAY, "time.year", "time.years"),
    (30 * DAY, "time.month", "time.months"),
    (7 * DAY, "time.week", "time.weeks"),
    (DAY, "time.day", "time.days"),
    (HOUR, "time.hour", "time.hours"),
];

/// Formats a Unix time as its date and the time left until it, e.g. `2026-12-01 (2 months)`.
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

/// The current Unix time in seconds.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Names the remaining time in its largest whole unit.
fn time_left(locale: Locale, seconds: Option<u64>) -> String {
    let Some(seconds) = seconds.filter(|seconds| *seconds > 0) else {
        return locale.t("time.expired").to_string();
    };
    UNITS
        .iter()
        .find_map(|&(size, one, many)| {
            let count = seconds / size;
            let key = if count == 1 { one } else { many };
            (count > 0).then(|| locale.tf(key, &[("count", &count.to_string())]))
        })
        .unwrap_or_else(|| locale.t("time.less_than_hour").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_uses_the_largest_whole_unit() {
        assert_eq!(time_left(Locale::En, Some(400 * DAY)), "1 year");
        assert_eq!(time_left(Locale::En, Some(90 * DAY)), "3 months");
        assert_eq!(time_left(Locale::En, Some(20 * DAY)), "2 weeks");
        assert_eq!(time_left(Locale::En, Some(DAY)), "1 day");
        assert_eq!(time_left(Locale::Tr, Some(2 * HOUR + 5)), "2 saat");
        assert_eq!(time_left(Locale::Tr, Some(60)), "bir saatten az");
        assert_eq!(time_left(Locale::En, Some(0)), "Expired");
        assert_eq!(time_left(Locale::Tr, None), "Süresi doldu");
    }
}
