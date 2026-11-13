//! Sizes, times and rates in a listing's few characters.

use time::{OffsetDateTime, UtcOffset};

const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

/// `2 KB`, `763 MB`, `1.18 GB`: whole numbers below a gigabyte.
pub fn size(bytes: u64) -> String {
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024. && unit < UNITS.len() - 1 {
        v /= 1024.;
        unit += 1;
    }
    if unit >= 3 {
        format!("{v:.2} {}", UNITS[unit])
    } else {
        format!("{v:.0} {}", UNITS[unit])
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn local(ms: u64, offset: UtcOffset) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .ok()
        .map(|t| t.to_offset(offset))
}

/// `14:16` today, `Oct 28` this year, `Oct 28 2025` before; at `offset`.
pub fn modified_at(ms: u64, now: u64, offset: UtcOffset) -> String {
    let (Some(at), Some(today)) = (local(ms, offset), local(now, offset)) else {
        return String::new();
    };
    if at.date() == today.date() {
        return format!("{:02}:{:02}", at.hour(), at.minute());
    }
    let day = format!("{} {:02}", MONTHS[at.month() as usize - 1], at.day());
    if at.year() == today.year() {
        day
    } else {
        format!("{day} {}", at.year())
    }
}

pub fn offset() -> UtcOffset {
    UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC)
}

pub fn modified(ms: u64) -> String {
    modified_at(ms, parvi_app_kit::hub::now_ms(), offset())
}

/// How long ago, in a few letters: `now`, `4m`, `3h`, `2d`, then the date.
pub fn ago_at(ms: u64, now: u64, offset: UtcOffset) -> String {
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        86_400..604_800 => format!("{}d", s / 86_400),
        _ => modified_at(ms, now, offset),
    }
}

pub fn ago(ms: u64) -> String {
    ago_at(ms, parvi_app_kit::hub::now_ms(), offset())
}

/// `9 s left`, `4 min left`, `2 h left`.
pub fn left(seconds: f64) -> String {
    let s = seconds.max(0.).round() as u64;
    if s < 60 {
        format!("{s} s left")
    } else if s < 3600 {
        format!("{} min left", s.div_ceil(60))
    } else {
        format!("{} h left", s.div_ceil(3600))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(2048), "2 KB");
        assert_eq!(size(763 << 20), "763 MB");
        assert_eq!(size(1_267_000_000), "1.18 GB");
    }

    #[test]
    fn ages() {
        let now = 1_800_000_000_000;
        let o = UtcOffset::UTC;
        assert_eq!(ago_at(now - 5_000, now, o), "now");
        assert_eq!(ago_at(now - 4 * 60_000, now, o), "4m");
        assert_eq!(ago_at(now - 3 * 3_600_000, now, o), "3h");
        assert_eq!(ago_at(now - 2 * 86_400_000, now, o), "2d");
        assert_eq!(ago_at(now - 30 * 86_400_000, now, o), "Dec 16 2026");
    }

    #[test]
    fn times() {
        let utc = UtcOffset::UTC;
        let now = 1_762_610_000_000; // 2025-11-08 13:53 UTC
        assert_eq!(modified_at(now - 60_000, now, utc), "13:52");
        assert_eq!(modified_at(now - 10 * 86_400_000, now, utc), "Oct 29");
        assert_eq!(modified_at(now - 400 * 86_400_000, now, utc), "Oct 04 2024");
        assert_eq!(left(9.2), "9 s left");
        assert_eq!(left(130.), "3 min left");
    }
}
