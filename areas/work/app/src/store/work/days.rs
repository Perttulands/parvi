//! Days, as the strip counts them (CHROTE's `beadsSlice.ts`, the strip):
//! each row's local calendar days of creation, last update and close, and the
//! first day any project holds.

use super::{Board, Status};
use time::{Date, Duration};

const DAY_MS: i64 = 86_400_000;
/// A time before 2000 is a date written as zero, not a date.
const EARLIEST: u64 = 946_684_800_000;
/// The days the strip spans unless every day is asked for.
pub const RECENT_DAYS: usize = 60;

/// The local calendar day of `ms`, as days since 1970-01-01 of that
/// calendar, in a zone `offset` seconds east of UTC.
pub fn local_day(ms: u64, offset: i64) -> i32 {
    (ms as i64 + offset * 1000).div_euclid(DAY_MS) as i32
}

fn date(day: i32) -> Date {
    Date::from_calendar_date(1970, time::Month::January, 1).expect("the epoch")
        + Duration::days(day.into())
}

/// A day as the strip and the range read it: `Oct 21`.
pub fn day_label(day: i32) -> String {
    let d = date(day);
    let month = &format!("{}", d.month())[..3];
    format!("{month} {}", d.day())
}

/// The first day the strip draws, as an offset from `first`: the last
/// [`RECENT_DAYS`], every day when asked, and never later than the start
/// of a chosen range, so a range of old days shows them.
pub fn strip_start(days: usize, all: bool, range_from: Option<i32>, first: i32) -> usize {
    if all || days <= RECENT_DAYS {
        return 0;
    }
    let recent = days - RECENT_DAYS;
    match range_from {
        Some(from) => (from - first).clamp(0, recent as i32) as usize,
        None => recent,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Days {
    pub today: i32,
    /// The earliest creation day of any row, or today.
    pub first: i32,
    pub created: Vec<Option<i32>>,
    pub updated: Vec<Option<i32>>,
    /// A finished row with no close time closed on its last update.
    pub closed: Vec<Option<i32>>,
    /// The day each project was first seen: its earliest creation.
    pub project_first: Vec<Option<i32>>,
}

impl Days {
    pub fn of(board: &Board, now: u64, offset: &dyn Fn(u64) -> i64) -> Days {
        let day = |ms: Option<u64>| {
            ms.filter(|&m| m >= EARLIEST)
                .map(|m| local_day(m, offset(m)))
        };
        let today = local_day(now, offset(now));
        let mut d = Days {
            today,
            first: today,
            project_first: vec![None; board.projects.len()],
            ..Days::default()
        };
        for r in &board.rows {
            let created = day(r.created);
            if let Some(c) = created {
                d.first = d.first.min(c);
                let pf = &mut d.project_first[r.project];
                *pf = Some(pf.map_or(c, |p| p.min(c)));
            }
            let updated = day(r.updated);
            let closed = day(r.closed).or(if r.status == Status::Closed {
                updated
            } else {
                None
            });
            d.created.push(created);
            d.updated.push(updated);
            d.closed.push(closed);
        }
        d
    }

    /// Whether row `i` was created, updated or closed within `from..=to`.
    pub fn in_range(&self, i: usize, from: i32, to: i32) -> bool {
        [self.created[i], self.updated[i], self.closed[i]]
            .into_iter()
            .flatten()
            .any(|d| (from..=to).contains(&d))
    }
}
