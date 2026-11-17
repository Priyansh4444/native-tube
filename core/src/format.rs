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

/// 1_311_805 as "1.3M".
pub fn short_count(n: u64) -> String {
    let (value, suffix) = match n {
        0..=999 => return n.to_string(),
        1_000..=999_999 => (n as f64 / 1e3, "K"),
        1_000_000..=999_999_999 => (n as f64 / 1e6, "M"),
        _ => (n as f64 / 1e9, "B"),
    };
    if value >= 100.0 {
        format!("{value:.0}{suffix}")
    } else {
        format!("{value:.1}{suffix}").replace(".0", "")
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
    }

    #[test]
    fn ages() {
        assert_eq!(relative_time(5), "just now");
        assert_eq!(relative_time(3_600), "1 hour ago");
        assert_eq!(relative_time(3 * 86_400), "3 days ago");
    }
}
