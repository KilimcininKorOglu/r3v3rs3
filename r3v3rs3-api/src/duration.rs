//! The text form of a duration in the configuration and the API, such as `30s`, `1h 5m` or
//! `2days 3h`: a list of spans, each an integer with an optional fraction and a unit.
//!
//! The form is the one of the humantime crate, which earlier releases used, so every value
//! that a stored configuration holds reads the same and writes back the same text. The units
//! are `ns`, `us`, `ms`, `s`, `m`, `h`, `d`, `w`, `M` (30.44 days) and `y` (365.25 days) with
//! their long names, and the written form names years, months and days in full.

use serde::{Deserialize, Deserializer, Serializer};
use std::time::Duration;

const NANOS_PER_SEC: u128 = 1_000_000_000;
const MONTH_SECS: u64 = 2_630_016;
const YEAR_SECS: u64 = 31_557_600;

/// The length of one unit in nanoseconds.
fn unit_nanos(unit: &str) -> Option<u128> {
    let secs: u64 = match unit {
        "nanos" | "nsec" | "ns" => return Some(1),
        "usec" | "us" | "µs" => return Some(1_000),
        "millis" | "msec" | "ms" => return Some(1_000_000),
        "seconds" | "second" | "secs" | "sec" | "s" => 1,
        "minutes" | "minute" | "min" | "mins" | "m" => 60,
        "hours" | "hour" | "hr" | "hrs" | "h" => 3_600,
        "days" | "day" | "d" => 86_400,
        "weeks" | "week" | "wk" | "wks" | "w" => 604_800,
        "months" | "month" | "M" => MONTH_SECS,
        "years" | "year" | "yr" | "yrs" | "y" => YEAR_SECS,
        _ => return None,
    };
    Some(u128::from(secs) * NANOS_PER_SEC)
}

/// Splits the leading ASCII digits off `text`.
fn split_digits(text: &str) -> (&str, &str) {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    text.split_at(end)
}

/// The integer and the fraction of a span, in units: the numerator and the denominator.
fn parse_number(text: &str) -> Result<(u128, u128, u128, &str), String> {
    let (int, rest) = split_digits(text);
    if int.is_empty() {
        return Err(format!("expected a number in `{text}`"));
    }
    let int: u128 = int
        .parse()
        .map_err(|_| "the number is too large".to_string())?;
    let Some(rest) = rest.strip_prefix('.') else {
        return Ok((int, 0, 1, rest));
    };
    let (frac, rest) = split_digits(rest);
    if frac.is_empty() || frac.len() > 18 {
        return Err(format!("invalid fraction in `{text}`"));
    }
    let numerator: u128 = frac.parse().map_err(|_| "invalid fraction".to_string())?;
    Ok((int, numerator, 10u128.pow(frac.len() as u32), rest))
}

/// Parses one span such as `1.5h` and returns its length and the text after it.
fn parse_span(text: &str) -> Result<(u128, &str), String> {
    let (int, numerator, denominator, rest) = parse_number(text)?;
    let rest = rest.trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_alphabetic() || c == 'µ'))
        .unwrap_or(rest.len());
    let (unit, rest) = rest.split_at(end);
    let per_unit = unit_nanos(unit).ok_or_else(|| format!("unknown time unit `{unit}`"))?;
    let nanos = int
        .checked_mul(per_unit)
        .and_then(|whole| whole.checked_add(numerator * per_unit / denominator))
        .ok_or_else(|| "the duration is too large".to_string())?;
    Ok((nanos, rest))
}

/// Parses the text form of a duration.
pub fn parse(text: &str) -> Result<Duration, String> {
    let mut rest = text.trim();
    if rest.is_empty() {
        return Err("the duration is empty".into());
    }
    let mut nanos = 0u128;
    while !rest.is_empty() {
        let (span, tail) = parse_span(rest)?;
        nanos = nanos
            .checked_add(span)
            .ok_or_else(|| "the duration is too large".to_string())?;
        rest = tail.trim_start();
    }
    let secs = u64::try_from(nanos / NANOS_PER_SEC)
        .map_err(|_| "the duration is too large".to_string())?;
    Ok(Duration::new(secs, (nanos % NANOS_PER_SEC) as u32))
}

/// Appends `value` with `name`, separated from an earlier span by a space.
fn push_span(out: &mut String, value: u64, name: &str, plural: bool) {
    if value == 0 {
        return;
    }
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(&value.to_string());
    out.push_str(name);
    if plural && value > 1 {
        out.push('s');
    }
}

/// Writes the text form of a duration, such as `1day 2h 3m` or `0s`.
pub fn format(duration: Duration) -> String {
    let secs = duration.as_secs();
    let nanos = u64::from(duration.subsec_nanos());
    if secs == 0 && nanos == 0 {
        return "0s".into();
    }
    let in_year = secs % YEAR_SECS;
    let in_month = in_year % MONTH_SECS;
    let in_day = in_month % 86_400;
    let mut out = String::new();
    push_span(&mut out, secs / YEAR_SECS, "year", true);
    push_span(&mut out, in_year / MONTH_SECS, "month", true);
    push_span(&mut out, in_month / 86_400, "day", true);
    push_span(&mut out, in_day / 3_600, "h", false);
    push_span(&mut out, in_day % 3_600 / 60, "m", false);
    push_span(&mut out, in_day % 60, "s", false);
    push_span(&mut out, nanos / 1_000_000, "ms", false);
    push_span(&mut out, nanos / 1_000 % 1_000, "us", false);
    push_span(&mut out, nanos % 1_000, "ns", false);
    out
}

/// Serializes a duration as its text form, for `#[serde(with = "crate::duration")]`.
pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format(*duration))
}

/// Deserializes a duration from its text form.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
    let text = String::deserialize(deserializer)?;
    parse(&text).map_err(serde::de::Error::custom)
}

/// The text form of an optional duration, for `#[serde(with = "crate::duration::option")]`.
pub mod option {
    use super::{format, parse};
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(
        duration: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match duration {
            Some(duration) => serializer.serialize_some(&format(*duration)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| parse(&text).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values that the humantime crate writes and reads, from its documentation and its
    /// own tests; a stored configuration holds these texts.
    #[test]
    fn the_text_form_matches_the_one_of_earlier_releases() {
        let cases = [
            (Duration::ZERO, "0s"),
            (Duration::from_millis(500), "500ms"),
            (Duration::from_secs(60), "1m"),
            (Duration::new(9420, 1_000_000), "2h 37m 1ms"),
            (Duration::from_secs(86_400), "1day"),
            (Duration::from_secs(2 * 86_400 + 3_661), "2days 1h 1m 1s"),
            (Duration::from_secs(2_630_016), "1month"),
            (Duration::from_secs(31_557_600), "1year"),
            (
                Duration::from_secs(2 * 31_557_600 + 3 * 2_630_016),
                "2years 3months",
            ),
            (Duration::new(0, 1_002_003), "1ms 2us 3ns"),
        ];
        for (duration, text) in cases {
            assert_eq!(format(duration), text);
            assert_eq!(parse(text), Ok(duration), "{text}");
        }
    }

    #[test]
    fn every_unit_name_and_a_fraction_read() {
        assert_eq!(parse("1hour 12min 5s"), Ok(Duration::from_secs(4325)));
        assert_eq!(parse(" 2 weeks "), Ok(Duration::from_secs(1_209_600)));
        assert_eq!(parse("1.5h"), Ok(Duration::from_secs(5400)));
        assert_eq!(parse("0.25s"), Ok(Duration::from_millis(250)));
        assert_eq!(parse("3M"), Ok(Duration::from_secs(3 * 2_630_016)));
        assert_eq!(parse("1µs"), Ok(Duration::from_micros(1)));
        assert_eq!(parse("10nsec"), Ok(Duration::from_nanos(10)));
    }

    /// A configuration value is outside input: an empty, malformed or oversized text must be
    /// refused, never read as zero.
    #[test]
    fn a_malformed_duration_is_refused() {
        for text in [
            "",
            "   ",
            "s",
            "5",
            "5x",
            "1.s",
            "-1s",
            "1s2",
            "1s,2m",
            "1e3s",
            "99999999999999999999999999999999999999999y",
            "600000000000y",
        ] {
            assert!(parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn an_optional_duration_reads_and_writes_null() {
        #[derive(serde_derive::Serialize, serde_derive::Deserialize, PartialEq, Debug)]
        struct Holder {
            #[serde(default, with = "option")]
            value: Option<Duration>,
        }
        let none: Holder = serde_json::from_str(r#"{"value":null}"#).unwrap();
        assert_eq!(none.value, None);
        let some: Holder = serde_json::from_str(r#"{"value":"1h"}"#).unwrap();
        assert_eq!(some.value, Some(Duration::from_secs(3600)));
        assert_eq!(serde_json::to_string(&some).unwrap(), r#"{"value":"1h"}"#);
        assert!(serde_json::from_str::<Holder>(r#"{"value":"soon"}"#).is_err());
    }
}
