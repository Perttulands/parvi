//! A day of points in fixed memory: the recent ones as read, and before them
//! one point per [`BUCKET`], the mean of the readings in it. A point leaving
//! the recent ring goes into its bucket; a bucket closes when the next one
//! starts. Both rings are allocated whole at start, so the history costs the
//! same after a minute and after a month.

use std::time::Duration;

use super::Ring;
use super::reading::Point;

/// How much time one earlier point stands for, in ms.
pub const BUCKET: u64 = 60_000;

pub struct History {
    recent: Ring<Point>,
    earlier: Ring<Point>,
    open: Option<Bucket>,
}

impl History {
    /// Keeps `recent` points as read and `earlier` buckets before them.
    pub fn new(recent: usize, earlier: usize) -> History {
        History {
            recent: Ring::new(recent),
            earlier: Ring::new(earlier),
            open: None,
        }
    }

    /// Keeps `recent` points read every `every`, and buckets before them to
    /// make up `span`, one of them still filling.
    pub fn spanning(span: Duration, every: Duration, recent: usize) -> History {
        let recent_ms = every.as_millis() as u64 * recent as u64;
        let span_ms = span.as_millis() as u64;
        let earlier = (span_ms.saturating_sub(recent_ms) / BUCKET).saturating_sub(1);
        History::new(recent, earlier as usize)
    }

    pub fn push(&mut self, point: Point) {
        let Some(old) = self.recent.push(point) else {
            return;
        };
        let start = old.at - old.at % BUCKET;
        match &mut self.open {
            Some(bucket) if bucket.start == start => bucket.add(&old),
            open => {
                if let Some(closed) = open.take() {
                    self.earlier.push(closed.point());
                }
                let mut bucket = Bucket::new(start);
                bucket.add(&old);
                *open = Some(bucket);
            }
        }
    }

    /// The points as read, oldest first.
    pub fn recent(&self) -> impl Iterator<Item = &Point> {
        self.recent.iter()
    }

    /// One point per closed bucket before the recent ones, oldest first. The
    /// bucket still filling is left out, so between these and the recent
    /// points lies at most a bucket's time.
    pub fn earlier(&self) -> impl Iterator<Item = &Point> {
        self.earlier.iter()
    }
}

/// The readings of one bucket so far: sums and counts of each figure.
struct Bucket {
    start: u64,
    sums: [f32; 6],
    counts: [u32; 6],
}

impl Bucket {
    fn new(start: u64) -> Bucket {
        Bucket {
            start,
            sums: [0.0; 6],
            counts: [0; 6],
        }
    }

    fn figures(p: &Point) -> [Option<f32>; 6] {
        [p.cpu, p.memory, p.gpu, p.load, p.swap, p.vram]
    }

    fn add(&mut self, p: &Point) {
        for (i, figure) in Bucket::figures(p).into_iter().enumerate() {
            if let Some(v) = figure {
                self.sums[i] += v;
                self.counts[i] += 1;
            }
        }
    }

    /// The bucket's point, at its start: each figure's mean, unknown where
    /// none was read.
    fn point(&self) -> Point {
        let mean = |i: usize| (self.counts[i] > 0).then(|| self.sums[i] / self.counts[i] as f32);
        Point {
            at: self.start,
            cpu: mean(0),
            memory: mean(1),
            gpu: mean(2),
            load: mean(3),
            swap: mean(4),
            vram: mean(5),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(at: u64, cpu: Option<f32>) -> Point {
        Point {
            at,
            cpu,
            memory: Some(50.0),
            ..Point::default()
        }
    }

    #[test]
    fn more_than_a_day_of_readings_stays_a_day_long() {
        // An hour of readings every 5 s, and minutes before it, one of them
        // still filling, to make up 24 hours.
        let day = Duration::from_secs(24 * 3600);
        let mut history = History::spanning(day, Duration::from_secs(5), 720);
        let every = 5_000;
        let readings = 26 * 3600 * 1000 / every;
        for i in 0..readings {
            history.push(at(i * every, Some((i % 10) as f32)));
        }
        let recent: Vec<_> = history.recent().collect();
        let earlier: Vec<_> = history.earlier().collect();
        assert_eq!(recent.len(), 720);
        assert_eq!(earlier.len(), 23 * 60 - 1);
        let newest = recent.last().unwrap().at;
        let oldest = earlier[0].at;
        assert!(newest - oldest <= 24 * 3600 * 1000, "{}", newest - oldest);
        assert!(earlier.windows(2).all(|w| w[1].at - w[0].at == BUCKET));
        assert!(earlier.last().unwrap().at + BUCKET <= recent[0].at);
        assert!(recent[0].at - earlier.last().unwrap().at <= 2 * BUCKET);
        // Twelve readings a minute, 0 to 9 over and over: a mean near 4.5.
        let mean = earlier[100].cpu.unwrap();
        assert!((3.5..=5.5).contains(&mean), "{mean}");
        assert_eq!(earlier[100].memory, Some(50.0));
    }

    #[test]
    fn a_bucket_means_what_was_read_and_nothing_read_stays_unknown() {
        let mut history = History::new(1, 4);
        history.push(at(0, Some(10.0)));
        history.push(at(1_000, Some(30.0)));
        history.push(at(2_000, None));
        // Pushing a point of the next minute out of `recent` closes the first.
        history.push(at(BUCKET, None));
        history.push(at(BUCKET + 1, None));
        let earlier: Vec<_> = history.earlier().copied().collect();
        assert_eq!(earlier.len(), 1);
        assert_eq!(earlier[0].at, 0);
        assert_eq!(earlier[0].cpu, Some(20.0));
        assert_eq!(earlier[0].gpu, None);
    }
}
