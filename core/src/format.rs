//! Small text formatters every frontend needs.

/// Seconds as "1:02:03" or "2:03". Negative and NaN inputs read as zero.
pub fn clock(seconds: f64) -> String {
    let total = if seconds.is_finite() && seconds > 0.0 {
        seconds as u64
    } else {
        0
    };
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// 1_311_805 as "1.3M". A value that would round to 1000 of a unit moves up a unit:
/// 999_500 is "1M", not "1000K".
pub fn short_count(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    let units = [(1e3, "K"), (1e6, "M"), (1e9, "B")];
    let mut i = match n {
        1_000..=999_999 => 0,
        1_000_000..=999_999_999 => 1,
        _ => 2,
    };
    loop {
        let (scale, suffix) = units[i];
        let value = n as f64 / scale;
        let shown = if value >= 100.0 {
            format!("{value:.0}")
        } else {
            format!("{value:.1}")
        };
        if i + 1 < units.len() && shown.parse::<f64>().unwrap_or(0.0) >= 1000.0 {
            i += 1;
            continue;
        }
        return format!("{}{suffix}", shown.strip_suffix(".0").unwrap_or(&shown));
    }
}

/// An age in seconds as "3 days ago".
pub fn relative_time(age_secs: i64) -> String {
    let age = age_secs.max(0);
    let (n, unit) = match age {
        0..=59 => return "just now".to_string(),
        60..=3_599 => (age / 60, "minute"),
        3_600..=86_399 => (age / 3_600, "hour"),
        86_400..=604_799 => (age / 86_400, "day"),
        604_800..=2_629_799 => (age / 604_800, "week"),
        2_629_800..=31_557_599 => (age / 2_629_800, "month"),
        _ => (age / 31_557_600, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// A publish date as YouTube words it. An ISO 8601 date-time ("2013-11-20T00:00:00Z", offsets
/// honoured) reads like [`relative_time`]; a date alone ("2013-11-20") reads the same way in
/// whole days, with "today" for the current UTC day. Anything else, such as YouTube's own
/// "3 days ago" or "Streamed 2 years ago", is returned unchanged. A future date reads as
/// "just now" (or "today").
pub fn published_label(text: &str, now_unix: i64) -> String {
    match iso_to_unix(text.trim()) {
        Some((at, true)) => relative_time(now_unix.saturating_sub(at)),
        Some((at, false)) => {
            let days = now_unix
                .div_euclid(86_400)
                .saturating_sub(at.div_euclid(86_400));
            if days <= 0 {
                "today".to_string()
            } else {
                relative_time(days.saturating_mul(86_400))
            }
        }
        None => text.to_string(),
    }
}

/// Unix seconds from "YYYY-MM-DD" or "YYYY-MM-DDThh:mm:ss[.fff][Z|±hh:mm]", and whether a
/// time of day was given. `None` for anything else.
pub(crate) fn iso_to_unix(s: &str) -> Option<(i64, bool)> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| {
        let part = s.get(r)?;
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse::<i64>().ok())?
    };
    if b.len() < 10 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    if b.len() == 10 {
        return Some((days * 86_400, false));
    }
    if !matches!(b.get(10), Some(b'T' | b't' | b' '))
        || b.get(13) != Some(&b':')
        || b.get(16) != Some(&b':')
    {
        return None;
    }
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let mut t = days * 86_400 + hh * 3600 + mm * 60 + ss;
    let tail = s.get(19..)?;
    if let Some(pos) = tail.find(['+', '-']) {
        let sign = if tail.get(pos..=pos) == Some("+") {
            1
        } else {
            -1
        };
        let off = tail.get(pos + 1..)?;
        let oh = off
            .get(0..2)
            .and_then(|x| x.parse::<i64>().ok())
            .unwrap_or(0);
        let om = off
            .get(3..5)
            .and_then(|x| x.parse::<i64>().ok())
            .unwrap_or(0);
        t -= sign * (oh * 3600 + om * 60);
    }
    Some((t, true))
}

/// Seconds since the Unix epoch, 0 if the clock is before it.
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(125.9), "2:05");
        assert_eq!(clock(3723.0), "1:02:03");
        assert_eq!(clock(f64::NAN), "0:00");
    }

    #[test]
    fn counts() {
        assert_eq!(short_count(999), "999");
        assert_eq!(short_count(1_311_805), "1.3M");
        assert_eq!(short_count(2_000), "2K");
        assert_eq!(short_count(123_456), "123K");
        assert_eq!(short_count(999_500), "1M");
        assert_eq!(short_count(999_999_000), "1B");
        assert_eq!(short_count(99_960), "100K");
    }

    #[test]
    fn ages() {
        assert_eq!(relative_time(5), "just now");
        assert_eq!(relative_time(3_600), "1 hour ago");
        assert_eq!(relative_time(3 * 86_400), "3 days ago");
    }

    #[test]
    fn published_labels() {
        // 2026-11-19T12:00:00Z
        let now = 1_795_089_600;
        assert_eq!(published_label("2013-11-20T00:00:00Z", now), "12 years ago");
        assert_eq!(published_label("2013-11-10", now), "13 years ago");
        assert_eq!(published_label("2026-11-19", now), "today");
        assert_eq!(published_label("2026-11-18", now), "1 day ago");
        assert_eq!(published_label("2026-11-16", now), "3 days ago");
        assert_eq!(published_label("2026-11-19T09:00:00Z", now), "3 hours ago");
        assert_eq!(
            published_label("2026-11-19T10:00:00+01:00", now),
            "3 hours ago"
        );
        assert_eq!(published_label("2026-11-25", now), "today");
        for passthrough in [
            "3 days ago",
            "Streamed 2 years ago",
            "",
            "2026-13-01",
            "2026-1-1",
        ] {
            assert_eq!(published_label(passthrough, now), passthrough);
        }
        assert_eq!(iso_to_unix("1970-01-01T00:00:00Z"), Some((0, true)));
        assert_eq!(iso_to_unix("1970-01-02"), Some((86_400, false)));
    }

    /// "today" and "just now" are 0; "N units ago" is N times the unit.
    fn rank(label: &str) -> Option<i64> {
        if label == "today" || label == "just now" {
            return Some(0);
        }
        let mut words = label.split(' ');
        let n: i64 = words.next()?.parse().ok()?;
        let unit = match words.next()?.trim_end_matches('s') {
            "minute" => 60,
            "hour" => 3_600,
            "day" => 86_400,
            "week" => 604_800,
            "month" => 2_629_800,
            "year" => 31_557_600,
            _ => return None,
        };
        (words.next() == Some("ago")).then_some(n * unit)
    }

    fn iso_date(unix: i64) -> String {
        // Inverse of the civil-day conversion (Howard Hinnant's civil_from_days).
        let z = unix.div_euclid(86_400) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}-{m:02}-{d:02}")
    }

    proptest::proptest! {
        #[test]
        fn published_label_never_panics(text in ".{0,40}", now in proptest::prelude::any::<i64>()) {
            let _ = published_label(&text, now);
        }

        #[test]
        fn published_label_is_monotone_in_age(
            now in 0i64..4_000_000_000,
            a in 0i64..3_000_000_000,
            b in 0i64..3_000_000_000,
        ) {
            let (young, old) = (a.min(b), a.max(b));
            for (younger, older) in [
                (published_label(&iso_date(now - young), now), published_label(&iso_date(now - old), now)),
                (relative_time(young), relative_time(old)),
            ] {
                let (r1, r2) = (rank(&younger), rank(&older));
                proptest::prop_assert!(r1.is_some() && r2.is_some(), "{younger:?} / {older:?}");
                proptest::prop_assert!(r1 <= r2, "{younger:?} for the younger, {older:?} for the older");
            }
            let back = iso_to_unix(&iso_date(now - young)).map(|(t, _)| t.div_euclid(86_400));
            proptest::prop_assert_eq!(back, Some((now - young).div_euclid(86_400)));
        }

        #[test]
        fn relative_labels_pass_through(age in 0i64..10_000_000_000, now in proptest::prelude::any::<i64>()) {
            let label = relative_time(age);
            proptest::prop_assert_eq!(published_label(&label, now), label.clone());
            let streamed = format!("Streamed {label}");
            proptest::prop_assert_eq!(published_label(&streamed, now), streamed.clone());
        }
    }
}
