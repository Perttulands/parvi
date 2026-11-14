//! Clock times as the app shows them: local, at the offset in force at
//! that moment (so an event from before a DST change keeps its hour).

use std::time::SystemTime;

/// `hh:mm` of a Unix ms moment, local time.
pub fn hhmm(ms: u64) -> String {
    let at = time::OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000);
    match at {
        Ok(utc) => format_local(utc),
        Err(_) => "--".into(),
    }
}

/// A past moment beside `now`, both Unix ms: `hh:mm` on the same local
/// day, else its day first (`12 Nov 23:48`), the year too when it differs.
pub fn stamp(ms: u64, now: u64) -> String {
    let local = |ms: u64| {
        let utc = time::OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000).ok()?;
        Some(
            time::UtcOffset::local_offset_at(utc)
                .map(|o| utc.to_offset(o))
                .unwrap_or(utc),
        )
    };
    let (Some(at), Some(now)) = (local(ms), local(now)) else {
        return "--".into();
    };
    day_and_time(at, now)
}

fn day_and_time(at: time::OffsetDateTime, now: time::OffsetDateTime) -> String {
    let clock = format!("{:02}:{:02}", at.hour(), at.minute());
    let month = &at.month().to_string()[..3];
    if at.date() == now.date() {
        clock
    } else if at.year() == now.year() {
        format!("{} {month} {clock}", at.day())
    } else {
        format!("{} {month} {} {clock}", at.day(), at.year())
    }
}

/// `hh:mm` of a moment, local time.
pub fn at(moment: SystemTime) -> String {
    format_local(time::OffsetDateTime::from(moment))
}

fn format_local(utc: time::OffsetDateTime) -> String {
    let local = time::UtcOffset::local_offset_at(utc)
        .map(|o| utc.to_offset(o))
        .unwrap_or(utc);
    format!("{:02}:{:02}", local.hour(), local.minute())
}

#[cfg(test)]
mod tests {
    use super::day_and_time;
    use time::{Date, Month, OffsetDateTime};

    fn at(year: i32, month: Month, day: u8, hour: u8, minute: u8) -> OffsetDateTime {
        Date::from_calendar_date(year, month, day)
            .unwrap()
            .with_hms(hour, minute, 0)
            .unwrap()
            .assume_utc()
    }

    #[test]
    fn a_moment_shows_its_day_only_when_it_is_not_today() {
        let now = at(2026, Month::November, 14, 9, 30);
        assert_eq!(
            day_and_time(at(2026, Month::November, 14, 0, 5), now),
            "00:05"
        );
        assert_eq!(
            day_and_time(at(2026, Month::November, 13, 23, 48), now),
            "13 Nov 23:48"
        );
        assert_eq!(
            day_and_time(at(2025, Month::December, 31, 23, 59), now),
            "31 Dec 2025 23:59"
        );
    }
}
