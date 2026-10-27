use r3v3rs3_api::i18n::Locale;
use std::cell::RefCell;
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};
use wasm_bindgen::JsCast;
use wasm_bindgen::UnwrapThrowExt;
use web_time::{SystemTime, UNIX_EPOCH};

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

thread_local! {
    /// The IANA time zone of the `[admin]` display_timezone setting, which the panel renders
    /// the stored times in. Empty is UTC, which is how the server stores every time.
    static TIMEZONE: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Sets the time zone that the panel renders the stored times in. Empty and UTC keep the
/// stored UTC times.
pub fn set_display_timezone(timezone: &str) {
    TIMEZONE.with(|cell| *cell.borrow_mut() = timezone.to_owned());
}

fn display_timezone() -> Option<String> {
    TIMEZONE.with(|cell| {
        let zone = cell.borrow().clone();
        (!zone.is_empty() && zone != "UTC").then_some(zone)
    })
}

/// Formats a Unix time as its date and the exact days left until it,
/// e.g. `2026-12-01 (59 days)`. A time less than a day away names its hours.
pub fn format_duration(locale: Locale, unix_time: i64) -> String {
    let timestamp = format_time(u64::try_from(unix_time).unwrap_throw() * 1000);
    let date = timestamp.split('T').next().unwrap_throw().to_owned();
    let remaining = u64::try_from(unix_time)
        .ok()
        .zip(u64::try_from(unix_now()).ok())
        .and_then(|(time, now)| time.checked_sub(now));
    format!("{date} ({})", time_left(locale, remaining))
}

/// A Unix time in milliseconds in the RFC 3339 form, in the display time zone when it is set
/// and in UTC otherwise.
pub fn format_time(ms: u64) -> String {
    match display_timezone().and_then(|zone| timezone_offset_seconds(&zone, ms)) {
        Some(offset) => rfc3339_in_offset(ms, offset).unwrap_or_else(|| utc_rfc3339(ms)),
        None => utc_rfc3339(ms),
    }
}

/// Formats a Unix timestamp in seconds as RFC 3339 in the display time zone, or "never" if non-positive.
pub fn format_unix_time(locale: Locale, unix_time: i64) -> String {
    if unix_time <= 0 {
        return locale.t("settings.never").to_string();
    }
    match u64::try_from(unix_time) {
        Ok(time) => format_time(time * 1000),
        Err(_) => unix_time.to_string(),
    }
}

/// An instant in the same form as `format_time`, in the display time zone when it is set and
/// in UTC otherwise. The server sends log timestamps as instants, not as Unix times.
pub fn format_instant(time: &OffsetDateTime) -> String {
    let ms = time.unix_timestamp_nanos() / 1_000_000;
    match u64::try_from(ms) {
        Ok(ms) => format_time(ms),
        Err(_) => time.format(&Rfc3339).unwrap_or_default(),
    }
}

/// The current Unix time in seconds.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// A Unix time in milliseconds in the RFC 3339 form, in UTC.
fn utc_rfc3339(ms: u64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
        .unwrap_or_else(|| ms.to_string())
}

/// A Unix time in milliseconds in the RFC 3339 form with the given offset in seconds east of
/// UTC. The instant stays the same; only the rendered wall clock moves.
fn rfc3339_in_offset(ms: u64, offset_seconds: i32) -> Option<String> {
    let time = OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()?;
    let sign = offset_seconds.signum();
    let rest = offset_seconds.abs();
    let offset = UtcOffset::from_hms(
        (sign * (rest / 3600)) as i8,
        (sign * ((rest % 3600) / 60)) as i8,
        (sign * (rest % 60)) as i8,
    )
    .ok()?;
    time.to_offset(offset).format(&Rfc3339).ok()
}

/// The offset of the IANA `zone` at the instant `ms`, in seconds east of UTC, as the browser
/// resolves it: the browser owns the zone database, so a daylight saving change at the instant
/// moves the offset with it. The formatter names the offset, for example `GMT+03:00`. Returns
/// nothing without a JavaScript runtime, which keeps the native tests on the UTC path.
fn timezone_offset_seconds(zone: &str, ms: u64) -> Option<i32> {
    let options = js_sys::Object::new();
    js_sys::Reflect::set(&options, &"timeZone".into(), &js_sys::JsString::from(zone)).ok()?;
    js_sys::Reflect::set(&options, &"hourCycle".into(), &"h23".into()).ok()?;
    js_sys::Reflect::set(&options, &"timeZoneName".into(), &"longOffset".into()).ok()?;
    let locales = js_sys::Array::of1(&"en-US".into());
    let formatter = js_sys::Intl::DateTimeFormat::new(&locales, &options);
    let parts = formatter.format_to_parts(&js_sys::Date::new(&js_sys::Number::from(ms as f64)));
    let name = parts.iter().find_map(|part| {
        let part = part.dyn_into::<js_sys::Object>().ok()?;
        let kind = js_sys::Reflect::get(&part, &"type".into())
            .ok()?
            .as_string()?;
        (kind == "timeZoneName").then_some(()).and_then(|()| {
            js_sys::Reflect::get(&part, &"value".into())
                .ok()?
                .as_string()
        })
    })?;
    Some(parse_gmt_offset(&name))
}

/// Reads the seconds east of UTC out of the `GMT+03:00` form that `longOffset` names. A bare
/// `GMT` is zero, and a zone without minutes keeps the `GMT+3` form.
fn parse_gmt_offset(name: &str) -> i32 {
    let Some(body) = name.strip_prefix("GMT") else {
        return 0;
    };
    if body.is_empty() {
        return 0;
    }
    let (sign, body) = if let Some(rest) = body.strip_prefix('-') {
        (-1, rest)
    } else {
        (1, body.strip_prefix('+').unwrap_or(body))
    };
    let (hours, minutes) = match body.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None => (body, "0"),
    };
    let hours: i32 = hours.parse().unwrap_or(0);
    let minutes: i32 = minutes.parse().unwrap_or(0);
    sign * (hours * 3600 + minutes * 60)
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

const KB: u64 = 1024;
const MB: u64 = 1024 * KB;
const GB: u64 = 1024 * MB;
const TB: u64 = 1024 * GB;

/// Formats a byte size into a human-readable string (B, KB, MB, GB, TB).
pub fn format_bytes(bytes: u64) -> String {
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else if bytes < GB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes < TB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    }
}

/// Formats milliseconds latency into a human-readable string (ms or s).
pub fn format_latency(ms: f64) -> String {
    if ms < 1.0 {
        format!("{ms:.2} ms")
    } else if ms < 1000.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

/// Formats an uptime in seconds into a human-readable duration (e.g. `2d 4h`, `3h 12m`, `45s`).
pub fn format_uptime(secs: u64) -> String {
    if secs >= 86400 {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    } else if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
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

    #[test]
    fn a_display_offset_shifts_the_rendered_wall_clock() {
        assert_eq!(utc_rfc3339(1_500), "1970-01-01T00:00:01.5Z");
        assert_eq!(
            rfc3339_in_offset(1_500, 3 * 3600),
            Some("1970-01-01T03:00:01.5+03:00".to_owned())
        );
        assert_eq!(
            rfc3339_in_offset(1_500, -5 * 3600),
            Some("1969-12-31T19:00:01.5-05:00".to_owned())
        );
    }

    #[test]
    fn format_bytes_scales_units() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[test]
    fn format_latency_scales_units() {
        assert_eq!(format_latency(0.42), "0.42 ms");
        assert_eq!(format_latency(15.2), "15.2 ms");
        assert_eq!(format_latency(1500.0), "1.50 s");
    }

    #[test]
    fn format_uptime_scales_units() {
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(125), "2m 5s");
        assert_eq!(format_uptime(3665), "1h 1m");
        assert_eq!(format_uptime(90000), "1d 1h");
    }

    #[test]
    fn format_unix_time_formats_seconds() {
        assert_eq!(format_unix_time(Locale::En, 100), "1970-01-01T00:01:40Z");
        assert_eq!(format_unix_time(Locale::En, 0), "never");
        assert_eq!(format_unix_time(Locale::Tr, 0), "henüz yok");
    }
}
