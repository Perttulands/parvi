//! When a task fires: an interval, or five cron fields read in an IANA time
//! zone, as CHROTE's `schedule.go` reads them (both day fields must match;
//! 7 is Sunday too).

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use parvi_scheduled_contract::Schedule;

use crate::Invalid;

/// Trims and defaults the fields, and checks that the trigger can name a
/// future run.
pub fn normalize(mut s: Schedule) -> Result<Schedule, Invalid> {
    s.kind = s.kind.trim().to_lowercase();
    s.expression = s.expression.trim().to_string();
    s.timezone = s.timezone.trim().to_string();
    s.duration = s.duration.trim().to_string();
    if s.timezone.is_empty() {
        s.timezone = "Local".into();
    }
    match s.kind.as_str() {
        "interval" => {
            interval(&s)?;
        }
        "cron" => {
            if s.expression.is_empty() {
                return Err(Invalid::new("cron expression is required"));
            }
            zone(&s.timezone)?;
            Cron::parse(&s.expression)?;
        }
        _ => return Err(Invalid::new("schedule type must be interval or cron")),
    }
    Ok(s)
}

/// The next firing strictly after `after`.
pub fn next_run(s: &Schedule, after: Timestamp) -> Result<Timestamp, Invalid> {
    let s = normalize(s.clone())?;
    if s.kind == "interval" {
        return after
            .checked_add(interval(&s)?)
            .map_err(|_| Invalid::new("interval is too long"));
    }
    let tz = zone(&s.timezone)?;
    let cron = Cron::parse(&s.expression)?;
    // The next whole minute, then minute by minute in absolute time, read in
    // the zone: a minute a clock change skips never matches, as in CHROTE.
    let minute = after.as_second().div_euclid(60) * 60 + 60;
    let mut at = Timestamp::from_second(minute).map_err(|_| Invalid::new("time out of range"))?;
    for _ in 0..5 * 366 * 24 * 60 {
        let local = at.to_zoned(tz.clone());
        if cron.matches(&local) {
            return Ok(at);
        }
        at = at
            .checked_add(SignedDuration::from_mins(1))
            .map_err(|_| Invalid::new("time out of range"))?;
    }
    Err(Invalid::new(
        "cron expression has no match in the next five years",
    ))
}

fn interval(s: &Schedule) -> Result<SignedDuration, Invalid> {
    if s.every_minutes < 0 {
        return Err(Invalid::new("interval everyMinutes must be positive"));
    }
    if s.every_minutes > 0 {
        return Ok(SignedDuration::from_mins(s.every_minutes));
    }
    if s.duration.is_empty() {
        return Err(Invalid::new(
            "interval everyMinutes or duration is required",
        ));
    }
    match go_duration(&s.duration) {
        Some(d) if d.is_positive() => Ok(d),
        _ => Err(Invalid::new(
            "interval duration must be a positive duration",
        )),
    }
}

/// The zone a schedule names; `Local` (or nothing) is the node's own.
pub fn zone(name: &str) -> Result<TimeZone, Invalid> {
    let name = name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("local") {
        return Ok(TimeZone::system());
    }
    TimeZone::get(name).map_err(|_| Invalid::new(format!("invalid timezone {name:?}")))
}

/// Go's `time.ParseDuration`: decimal numbers with units, `1h30m`, `90s`.
fn go_duration(text: &str) -> Option<SignedDuration> {
    let (negative, mut rest) = match text.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    if rest == "0" {
        return Some(SignedDuration::ZERO);
    }
    if rest.is_empty() {
        return None;
    }
    let mut nanos: f64 = 0.;
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let value: f64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit_end = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let scale = match &rest[..unit_end] {
            "ns" => 1.,
            "us" | "µs" | "μs" => 1e3,
            "ms" => 1e6,
            "s" => 1e9,
            "m" => 60e9,
            "h" => 3600e9,
            _ => return None,
        };
        rest = &rest[unit_end..];
        nanos += value * scale;
    }
    let nanos = if negative { -nanos } else { nanos };
    (nanos.abs() < 9.2e18).then(|| SignedDuration::from_nanos(nanos as i64))
}

struct Cron {
    minute: Field,
    hour: Field,
    day: Field,
    month: Field,
    week: Field,
}

/// `None` is `*`; otherwise the values that match.
struct Field(Option<Vec<bool>>);

impl Cron {
    fn parse(expression: &str) -> Result<Cron, Invalid> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(Invalid::new("cron expression must have five fields"));
        }
        let field = |i: usize, name: &str, min, max, sunday| {
            Field::parse(fields[i], min, max, sunday)
                .map_err(|e| Invalid::new(format!("invalid cron {name}: {e}")))
        };
        Ok(Cron {
            minute: field(0, "minute", 0, 59, false)?,
            hour: field(1, "hour", 0, 23, false)?,
            day: field(2, "day-of-month", 1, 31, false)?,
            month: field(3, "month", 1, 12, false)?,
            week: field(4, "day-of-week", 0, 7, true)?,
        })
    }

    fn matches(&self, t: &jiff::Zoned) -> bool {
        self.minute.has(t.minute() as usize)
            && self.hour.has(t.hour() as usize)
            && self.day.has(t.day() as usize)
            && self.month.has(t.month() as usize)
            && self.week.has(t.weekday().to_sunday_zero_offset() as usize)
    }
}

impl Field {
    fn has(&self, v: usize) -> bool {
        match &self.0 {
            None => true,
            Some(values) => values.get(v).copied().unwrap_or(false),
        }
    }

    fn parse(raw: &str, min: usize, max: usize, sunday: bool) -> Result<Field, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("empty field".into());
        }
        if raw == "*" {
            return Ok(Field(None));
        }
        let mut values = vec![false; max + 1];
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                return Err("empty list item".into());
            }
            let (base, step) = match token.split_once('/') {
                Some((base, step)) => {
                    let step: usize = step
                        .parse()
                        .ok()
                        .filter(|s| *s > 0)
                        .ok_or_else(|| format!("invalid step {token:?}"))?;
                    (base, step)
                }
                None => (token, 1),
            };
            let (start, end) = if base == "*" {
                (min, max)
            } else if let Some((a, b)) = base.split_once('-') {
                if a.is_empty() || b.is_empty() || b.contains('-') {
                    return Err(format!("invalid range {token:?}"));
                }
                let a = a
                    .parse()
                    .map_err(|_| format!("invalid range start {token:?}"))?;
                let b = b
                    .parse()
                    .map_err(|_| format!("invalid range end {token:?}"))?;
                (a, b)
            } else if !base.is_empty() {
                let v = base
                    .parse()
                    .map_err(|_| format!("invalid value {token:?}"))?;
                (v, v)
            } else {
                return Err(format!("invalid token {token:?}"));
            };
            if start < min || start > max || end < min || end > max || start > end {
                return Err(format!("value out of range {token:?}"));
            }
            for v in (start..=end).step_by(step) {
                values[if sunday && v == 7 { 0 } else { v }] = true;
            }
        }
        Ok(Field(Some(values)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn sched(kind: &str, expression: &str, every: i64) -> Schedule {
        Schedule {
            kind: kind.into(),
            expression: expression.into(),
            timezone: "UTC".into(),
            every_minutes: every,
            duration: String::new(),
        }
    }

    /// CHROTE's TestNextRunComputesIntervalAndCron.
    #[test]
    fn next_run_computes_interval_and_cron() {
        let after = at("2026-06-27T14:29:30Z");
        assert_eq!(
            next_run(&sched("interval", "", 15), after).unwrap(),
            at("2026-06-27T14:44:30Z")
        );
        let cron = sched("cron", "30 14 * * *", 0);
        assert_eq!(next_run(&cron, after).unwrap(), at("2026-06-27T14:30:00Z"));
        assert_eq!(
            next_run(&cron, at("2026-06-27T14:30:00Z")).unwrap(),
            at("2026-06-28T14:30:00Z")
        );
    }

    /// CHROTE's TestValidateScheduleRejectsInvalidIntervalAndCron.
    #[test]
    fn validate_schedule_rejects_invalid_interval_and_cron() {
        let mut bad_duration = sched("interval", "", 0);
        bad_duration.duration = "soon".into();
        for s in [
            sched("interval", "", 0),
            sched("interval", "", -5),
            bad_duration,
            sched("cron", "* * * *", 0),
            sched("cron", "61 * * * *", 0),
            sched("solar", "sunrise", 0),
        ] {
            assert!(
                next_run(&s, at("2026-06-27T14:00:00Z")).is_err(),
                "{s:?} is refused"
            );
        }
    }

    #[test]
    fn cron_reads_its_zone_ranges_steps_and_sunday_as_seven() {
        let mut helsinki = sched("cron", "0 16 * * *", 0);
        helsinki.timezone = "Europe/Helsinki".into();
        // 16:00 in Helsinki in summer is 13:00 UTC.
        assert_eq!(
            next_run(&helsinki, at("2026-06-27T12:00:00Z")).unwrap(),
            at("2026-06-27T13:00:00Z")
        );
        // 2026-06-28 is a Sunday.
        assert_eq!(
            next_run(&sched("cron", "0 9 * * 7", 0), at("2026-06-27T00:00:00Z")).unwrap(),
            at("2026-06-28T09:00:00Z")
        );
        assert_eq!(
            next_run(
                &sched("cron", "*/20 9-10 * * 1-5", 0),
                at("2026-06-29T09:41:00Z")
            )
            .unwrap(),
            at("2026-06-29T10:00:00Z")
        );
        assert!(next_run(&sched("cron", "0 0 31 2 *", 0), at("2026-01-01T00:00:00Z")).is_err());
        let mut nowhere = helsinki.clone();
        nowhere.timezone = "Mars/Olympus".into();
        assert!(normalize(nowhere).is_err());
    }

    #[test]
    fn an_interval_may_be_a_go_duration() {
        let mut s = sched("interval", "", 0);
        s.duration = "1h30m".into();
        assert_eq!(
            next_run(&s, at("2026-06-27T14:00:00Z")).unwrap(),
            at("2026-06-27T15:30:00Z")
        );
        assert_eq!(go_duration("1.5s"), Some(SignedDuration::from_millis(1500)));
        assert_eq!(go_duration("10"), None);
    }
}
