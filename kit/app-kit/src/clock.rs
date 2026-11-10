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
