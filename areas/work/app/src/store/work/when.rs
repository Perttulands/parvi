//! The trackers' ISO 8601 times, read into milliseconds since the epoch,
//! and said the way the Work tab says them.

use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

/// `2026-11-08T13:53:00Z`, with or without fractions, `Z` or `±hh:mm`.
pub fn parse(s: &str) -> Option<u64> {
    let s = s.trim();
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<u32>().ok();
    let date = Date::from_calendar_date(
        num(0, 4)? as i32,
        Month::try_from(num(5, 7)? as u8).ok()?,
        num(8, 10)? as u8,
    )
    .ok()?;
    let (h, m, sec) = if s.len() >= 19 {
        (num(11, 13)?, num(14, 16)?, num(17, 19)?)
    } else {
        (0, 0, 0)
    };
    let mut rest = s.get(19..).unwrap_or("");
    let mut nanos = 0u32;
    if let Some(f) = rest.strip_prefix('.') {
        let digits: String = f.chars().take_while(|c| c.is_ascii_digit()).collect();
        rest = &f[digits.len()..];
        let d: String = digits
            .chars()
            .chain(std::iter::repeat('0'))
            .take(9)
            .collect();
        nanos = d.parse().ok()?;
    }
    let offset = match rest {
        "" | "Z" | "z" => UtcOffset::UTC,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let hh: i8 = o.get(1..3)?.parse().ok()?;
            let mm: i8 = o
                .get(4..6)
                .or_else(|| o.get(3..5))
                .unwrap_or("0")
                .parse()
                .ok()?;
            UtcOffset::from_hms(sign * hh, sign * mm, 0).ok()?
        }
    };
    let t = Time::from_hms_nano(h as u8, m as u8, sec as u8, nanos).ok()?;
    let at: OffsetDateTime = PrimitiveDateTime::new(date, t).assume_offset(offset);
    u64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()
}

/// Seconds east of UTC in this machine's zone at `ms`.
pub fn local_offset(ms: u64) -> i64 {
    OffsetDateTime::from_unix_timestamp((ms / 1000) as i64)
        .ok()
        .and_then(|at| UtcOffset::local_offset_at(at).ok())
        .map_or(0, |o| o.whole_seconds().into())
}

/// How long ago, as the footer says it: `moments`, `40 min`, `3 h`,
/// `2 days` (CHROTE's `ageLabel`).
pub fn age(ms: u64, now: u64) -> String {
    let minutes = now.saturating_sub(ms) / 60_000;
    match minutes {
        0 => "moments".into(),
        1..60 => format!("{minutes} min"),
        _ if minutes / 60 < 48 => format!("{} h", minutes / 60),
        _ => format!("{} days", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_times_read() {
        assert_eq!(parse("2025-11-08T13:53:20Z"), Some(1_762_610_000_000));
        assert_eq!(parse("2025-11-08T15:53:20+02:00"), Some(1_762_610_000_000));
        assert_eq!(parse("2025-11-08T13:53:20.5Z"), Some(1_762_610_000_500));
        assert_eq!(
            parse("2025-11-08T08:53:20.123456789-05:00"),
            Some(1_762_610_000_123)
        );
        assert_eq!(parse("soon"), None);
    }

    #[test]
    fn ages() {
        let now = 1_762_610_000_000;
        assert_eq!(age(now - 20_000, now), "moments");
        assert_eq!(age(now - 40 * 60_000, now), "40 min");
        assert_eq!(age(now - 3 * 3_600_000, now), "3 h");
        assert_eq!(age(now - 3 * 86_400_000, now), "3 days");
    }
}
