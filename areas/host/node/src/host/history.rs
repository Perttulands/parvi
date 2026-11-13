//! A day of the host in fixed memory: one point per minute, the minute still
//! filling last. A reading joins its minute; the minute closes when a
//! reading of a later one comes. The ring is allocated whole at start, so
//! the history costs the same after a minute and after a month.

use super::Ring;
use parvi_host_contract::{Minute, Point};

/// Minutes kept: a day.
pub const MINUTES: usize = 24 * 60;

pub struct History {
    closed: Ring<Point>,
    open: Option<Minute>,
}

impl History {
    /// Keeps `minutes` closed minutes before the one filling.
    pub fn new(minutes: usize) -> History {
        History {
            closed: Ring::new(minutes),
            open: None,
        }
    }

    pub fn push(&mut self, point: &Point) {
        match &mut self.open {
            Some(minute) if minute.holds(point.at) => minute.add(point),
            open => {
                if let Some(done) = open.take() {
                    self.closed.push(done.point());
                }
                let mut minute = Minute::starting(point.at);
                minute.add(point);
                *open = Some(minute);
            }
        }
    }

    /// Every minute, oldest first, the one filling last.
    pub fn minutes(&self) -> Vec<Point> {
        self.closed
            .iter()
            .cloned()
            .chain(self.open.as_ref().map(Minute::point))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_host_contract::MINUTE;

    fn at(at: u64, cpu: f32) -> Point {
        Point {
            at,
            cpu: Some(cpu),
            ..Point::default()
        }
    }

    #[test]
    fn more_than_a_day_of_readings_stays_a_day_long() {
        let mut history = History::new(MINUTES);
        let every = 5_000;
        for i in 0..(26 * 3600 * 1000 / every) {
            history.push(&at(i * every, (i % 10) as f32));
        }
        let minutes = history.minutes();
        assert_eq!(minutes.len(), MINUTES + 1, "a day, and the minute filling");
        assert!(minutes.windows(2).all(|w| w[1].at - w[0].at == MINUTE));
        // Twelve readings a minute, 0 to 9 over and over: a mean near 4.5.
        let mean = minutes[100].cpu.unwrap();
        assert!((3.5..=5.5).contains(&mean), "{mean}");
    }

    #[test]
    fn a_reading_joins_its_minute_and_a_silence_leaves_a_gap() {
        let mut history = History::new(8);
        history.push(&at(0, 10.));
        history.push(&at(30_000, 30.));
        assert_eq!(history.minutes().len(), 1);
        assert_eq!(history.minutes()[0].cpu, Some(20.), "the minute so far");
        history.push(&at(5 * MINUTE + 1, 50.));
        let minutes = history.minutes();
        assert_eq!(
            minutes.iter().map(|p| p.at).collect::<Vec<_>>(),
            [0, 5 * MINUTE]
        );
    }
}
