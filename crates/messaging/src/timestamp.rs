//! Discord `<t:unix:style>` rendering (en-US).

use std::fmt::Display;

use chrono::{DateTime, Datelike, Local, TimeZone, Utc};

/// Formats `unix` (seconds) in the local timezone.
///
/// Styles: `t` short time, `T` long time, `d` short date, `D` long date, `f` date + short time,
/// `F` weekday + date + short time, `R` relative to `now_unix`. Unknown styles behave like `f`.
pub fn format(unix: i64, style: char, now_unix: i64, use_24h: bool) -> String {
    format_in(&Local, unix, style, now_unix, use_24h)
}

/// Same as [`format`] with an explicit timezone.
pub fn format_in<Tz>(tz: &Tz, unix: i64, style: char, now_unix: i64, use_24h: bool) -> String
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    if style == 'R' {
        return relative(unix, now_unix);
    }
    // Years beyond 9999 are clamped out so a zone offset can never overflow chrono's range.
    let Some(utc) = DateTime::<Utc>::from_timestamp(unix, 0).filter(|d| (0..=9999).contains(&d.year())) else {
        return unix.to_string();
    };
    let dt = utc.with_timezone(tz);
    let short = if use_24h { "%H:%M" } else { "%-I:%M %p" };
    let long = if use_24h { "%H:%M:%S" } else { "%-I:%M:%S %p" };
    let pattern = match style {
        't' => short.to_string(),
        'T' => long.to_string(),
        'd' => "%m/%d/%Y".to_string(),
        'D' => "%B %-d, %Y".to_string(),
        'F' => format!("%A, %B %-d, %Y {short}"),
        _ => format!("%B %-d, %Y {short}"),
    };
    dt.format(&pattern).to_string()
}

fn relative(unix: i64, now: i64) -> String {
    let delta = unix.saturating_sub(now);
    let secs = delta.unsigned_abs();
    let phrase = if secs == 0 {
        return "now".to_string();
    } else if secs < 60 {
        plural(secs, "second")
    } else if secs < 3_600 {
        plural(secs / 60, "minute")
    } else if secs < 86_400 {
        plural(secs / 3_600, "hour")
    } else if secs < 30 * 86_400 {
        plural(secs / 86_400, "day")
    } else if secs < 365 * 86_400 {
        plural(secs / (30 * 86_400), "month")
    } else {
        plural(secs / (365 * 86_400), "year")
    };
    if delta > 0 {
        format!("in {phrase}")
    } else {
        format!("{phrase} ago")
    }
}

fn plural(n: u64, unit: &str) -> String {
    match (n, unit) {
        (1, "hour") => "an hour".to_string(),
        (1, _) => format!("a {unit}"),
        _ => format!("{n} {unit}s"),
    }
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    // 2021-04-20 16:20:30 UTC
    const T: i64 = 1_618_935_630;

    #[test]
    fn absolute_styles_24h() {
        let utc = Utc;
        assert_eq!(format_in(&utc, T, 't', 0, true), "16:20");
        assert_eq!(format_in(&utc, T, 'T', 0, true), "16:20:30");
        assert_eq!(format_in(&utc, T, 'd', 0, true), "04/20/2021");
        assert_eq!(format_in(&utc, T, 'D', 0, true), "April 20, 2021");
        assert_eq!(format_in(&utc, T, 'f', 0, true), "April 20, 2021 16:20");
        assert_eq!(
            format_in(&utc, T, 'F', 0, true),
            "Tuesday, April 20, 2021 16:20"
        );
    }

    #[test]
    fn absolute_styles_12h() {
        let utc = Utc;
        assert_eq!(format_in(&utc, T, 't', 0, false), "4:20 PM");
        assert_eq!(format_in(&utc, T, 'T', 0, false), "4:20:30 PM");
        assert_eq!(format_in(&utc, T, 'f', 0, false), "April 20, 2021 4:20 PM");
        assert_eq!(format_in(&utc, T - 16 * 3600, 't', 0, false), "12:20 AM");
    }

    #[test]
    fn unknown_style_is_f_and_offsets_apply() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(format_in(&tz, T, 'x', 0, true), "April 20, 2021 18:20");
    }

    #[test]
    fn relative_styles() {
        let now = 1_700_000_000;
        assert_eq!(format(now, 'R', now, true), "now");
        assert_eq!(format(now - 1, 'R', now, true), "a second ago");
        assert_eq!(format(now - 30, 'R', now, true), "30 seconds ago");
        assert_eq!(format(now - 60, 'R', now, true), "a minute ago");
        assert_eq!(format(now + 300, 'R', now, true), "in 5 minutes");
        assert_eq!(format(now - 3600, 'R', now, true), "an hour ago");
        assert_eq!(format(now + 7200, 'R', now, true), "in 2 hours");
        assert_eq!(format(now - 86_400, 'R', now, true), "a day ago");
        assert_eq!(format(now - 3 * 86_400, 'R', now, true), "3 days ago");
        assert_eq!(format(now - 30 * 86_400, 'R', now, true), "a month ago");
        assert_eq!(format(now + 90 * 86_400, 'R', now, true), "in 3 months");
        assert_eq!(format(now - 365 * 86_400, 'R', now, true), "a year ago");
        assert_eq!(
            format(now - 3 * 365 * 86_400, 'R', now, true),
            "3 years ago"
        );
    }

    #[test]
    fn extreme_values_do_not_panic() {
        assert_eq!(format(i64::MAX, 'f', 0, true), i64::MAX.to_string());
        assert!(format(i64::MIN, 'R', i64::MAX, true).ends_with("ago"));
        let _ = format(i64::MIN, 'F', 0, false);
    }
}
