//! Each node's host, as its host channel tells it (docs/protocol.md, Host
//! channel), and the rules CHROTE's Server tab draws it by: each row scaled
//! to its own peak, a gap never bridged, window stats, the time axis, and
//! the figures' words. Plain data; the Fleet page draws it.

pub use parvi_host_contract::State;
use parvi_host_contract::{Event, EventFrame, Reading, ReadingFrame};
use parvi_host_contract::{HOST_EVENT as EVENT, HOST_READING as READING, HOST_STATE as STATE};
pub use parvi_host_contract::{Point, Usage};
use std::collections::HashMap;

/// Events the app keeps per node: the node keeps more.
pub const EVENTS_KEPT: usize = 50;

/// What every node's host channel said last. A node that goes away keeps
/// its figures, so its room shows what it last knew.
#[derive(Default)]
pub struct Hosts {
    pub nodes: HashMap<String, State>,
    open: HashMap<String, Bucket>,
}

impl Hosts {
    /// Takes a host frame from `node`, giving back a new event, which the
    /// status line tells. `None` if the frame was not Host's.
    pub fn take(&mut self, node: &str, kind: u8, body: &[u8]) -> Option<Option<Event>> {
        if kind == EVENT {
            let e = serde_json::from_slice::<EventFrame>(body)
                .ok()
                .map(|f| f.event);
            if let (Some(e), Some(s)) = (&e, self.nodes.get_mut(node)) {
                s.events.push(e.clone());
                let over = s.events.len().saturating_sub(EVENTS_KEPT);
                s.events.drain(..over);
            }
            return Some(e);
        }
        self.state_or_reading(node, kind, body).then_some(None)
    }

    fn state_or_reading(&mut self, node: &str, kind: u8, body: &[u8]) -> bool {
        match kind {
            STATE => {
                if let Ok(mut s) = serde_json::from_slice::<State>(body) {
                    let over = s.events.len().saturating_sub(EVENTS_KEPT);
                    s.events.drain(..over);
                    self.nodes.insert(node.to_string(), s);
                    self.open.remove(node);
                }
                true
            }
            READING => {
                if let Ok(r) = serde_json::from_slice::<ReadingFrame>(body)
                    && let Some(s) = self.nodes.get_mut(node)
                {
                    append(s, &mut self.open, node, r.point, r.reading);
                }
                true
            }
            _ => false,
        }
    }
}

/// An hour: how long points stay as read, as the node keeps them.
pub const HOUR: u64 = 3_600_000;
/// The longest the charts look back, as far as the node keeps.
pub const DAY: u64 = 24 * HOUR;

/// A new point joins `history`; one that is now older than an hour goes
/// into its bucket, as the node's history does (`src/host/history.rs`), and
/// what is older than a day goes.
fn append(
    s: &mut State,
    open: &mut HashMap<String, Bucket>,
    node: &str,
    point: Point,
    reading: Reading,
) {
    s.history.retain(|p| p.at < point.at);
    s.history.push(point);
    let aged = s
        .history
        .iter()
        .take_while(|p| p.at + HOUR <= point.at)
        .count();
    for old in s.history.drain(..aged).collect::<Vec<_>>() {
        fold(s, open, node, old);
    }
    s.earlier.retain(|p| p.at + DAY > point.at);
    s.latest = Some(reading);
}

fn fold(s: &mut State, open: &mut HashMap<String, Bucket>, node: &str, p: Point) {
    let width = s.bucket.max(1);
    let start = p.at - p.at % width;
    let bucket = open.entry(node.to_owned()).or_insert_with(|| Bucket {
        start,
        ..Bucket::default()
    });
    if bucket.start != start {
        s.earlier.push(bucket.point());
        *bucket = Bucket {
            start,
            ..Bucket::default()
        };
    }
    bucket.add(&p);
}

/// The figures of one bucket so far.
#[derive(Clone, Debug, Default, PartialEq)]
struct Bucket {
    start: u64,
    sums: [f32; 6],
    counts: [u32; 6],
}

impl Bucket {
    fn add(&mut self, p: &Point) {
        for (i, m) in Metric::ALL.iter().enumerate() {
            if let Some(v) = m.of(p) {
                self.sums[i] += v;
                self.counts[i] += 1;
            }
        }
    }

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

/// Which figure of a point a row draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Cpu,
    Memory,
    Gpu,
    Load,
    Swap,
    Vram,
}

impl Metric {
    const ALL: [Metric; 6] = [
        Metric::Cpu,
        Metric::Memory,
        Metric::Gpu,
        Metric::Load,
        Metric::Swap,
        Metric::Vram,
    ];

    pub fn of(self, p: &Point) -> Option<f32> {
        match self {
            Metric::Cpu => p.cpu,
            Metric::Memory => p.memory,
            Metric::Gpu => p.gpu,
            Metric::Load => p.load,
            Metric::Swap => p.swap,
            Metric::Vram => p.vram,
        }
    }
}

pub trait StateExt {
    fn since(&self, start: u64) -> impl Iterator<Item = &Point> + Clone;
    fn oldest(&self) -> Option<u64>;
    fn runs(&self, metric: Metric, start: u64) -> Vec<Vec<(u64, f32)>>;
}

impl StateExt for State {
    /// Every point from `start` on, oldest first: the buckets, then the
    /// points as read.
    fn since(&self, start: u64) -> impl Iterator<Item = &Point> + Clone {
        self.earlier
            .iter()
            .chain(&self.history)
            .filter(move |p| p.at >= start)
    }

    /// The oldest moment this node's history reaches.
    fn oldest(&self) -> Option<u64> {
        self.earlier.first().or(self.history.first()).map(|p| p.at)
    }

    /// The row's line from `start` on: the buckets' runs, then the points'
    /// runs, the two joined where less than three buckets lie between them.
    fn runs(&self, metric: Metric, start: u64) -> Vec<Vec<(u64, f32)>> {
        let since = |points: &[Point]| -> Vec<Point> {
            points.iter().filter(|p| p.at >= start).copied().collect()
        };
        let mut out = runs_with_gap(&since(&self.earlier), metric, self.bucket.max(1));
        let mut recent = runs(&since(&self.history), metric, self.every).into_iter();
        if let Some(first) = recent.next() {
            let end = out.last().and_then(|run| run.last()).map(|(at, _)| *at);
            let begin = first.first().map(|(at, _)| *at);
            match (out.last_mut(), end.zip(begin)) {
                (Some(last), Some((end, begin)))
                    if self.earlier.last().is_some_and(|p| metric.of(p).is_some())
                        && self.history.first().is_some_and(|p| metric.of(p).is_some())
                        && begin.saturating_sub(end) <= self.bucket.max(1000) * 3 =>
                {
                    last.extend(first)
                }
                _ => out.push(first),
            }
        }
        out.extend(recent);
        out
    }
}

/// How far back the charts reach: as far as any node's history does, at
/// least an hour and at most a day.
pub fn span(oldest: Option<u64>, end: u64) -> u64 {
    oldest
        .map_or(HOUR, |o| end.saturating_sub(o))
        .clamp(HOUR, DAY)
}

/// Steps the time axis may take, in minutes: the smallest that leaves at
/// most six ticks.
const STEPS: [u64; 8] = [10, 15, 30, 60, 120, 180, 240, 360];

/// The time axis's ticks between `start` and `end` (Unix ms), on whole
/// steps of the local clock `offset` seconds from UTC, none so near `end`
/// that it would crowd the now edge.
pub fn ticks(start: u64, end: u64, offset: i64) -> Vec<u64> {
    let span = end.saturating_sub(start);
    let step = STEPS
        .iter()
        .map(|m| m * 60_000)
        .find(|step| span / step <= 6)
        .unwrap_or(6 * HOUR);
    let local = |at: u64| at as i64 + offset * 1000;
    let first = {
        let l = local(start);
        let rounded = l.div_euclid(step as i64) * step as i64 + step as i64;
        (rounded - offset * 1000) as u64
    };
    let clear = span / 12;
    (0..)
        .map(|i| first + i * step)
        .take_while(|at| *at + clear < end)
        .collect()
}

/// The row's top: its peak and a quarter, to the next 5%, between 5 and
/// 100 (CHROTE `SystemStatusView` `ceiling`). An all-zero row sits on a 5%
/// floor; a row with nothing is 100.
pub fn ceiling(values: impl Iterator<Item = f32>) -> f32 {
    let mut any = false;
    let peak = values.fold(0f32, |m, v| {
        any = true;
        m.max(v)
    });
    if !any {
        return 100.;
    }
    ((peak * 1.25 / 5.).ceil() * 5.).clamp(5., 100.)
}

/// Peak and mean over every figure the window has.
pub fn stats(values: impl Iterator<Item = f32>) -> Option<(f32, f32)> {
    let (mut n, mut sum, mut peak) = (0u32, 0f32, f32::MIN);
    for v in values {
        n += 1;
        sum += v;
        peak = peak.max(v);
    }
    (n > 0).then(|| (peak, sum / n as f32))
}

/// The row's line as runs of (at, value). A missing figure breaks it, and
/// so does a silence longer than three readings: the node was away or
/// restarted, and nothing is drawn where nothing was read.
pub fn runs(points: &[Point], metric: Metric, every: u64) -> Vec<Vec<(u64, f32)>> {
    runs_with_gap(points, metric, every.max(1000) * 3)
}

fn runs_with_gap(points: &[Point], metric: Metric, gap: u64) -> Vec<Vec<(u64, f32)>> {
    let mut out: Vec<Vec<(u64, f32)>> = vec![];
    let mut last: Option<u64> = None;
    for p in points {
        let Some(v) = metric.of(p) else {
            last = None;
            continue;
        };
        let joined = last.is_some_and(|l| p.at.saturating_sub(l) <= gap);
        match out.last_mut() {
            Some(run) if joined => run.push((p.at, v)),
            _ => out.push(vec![(p.at, v)]),
        }
        last = Some(p.at);
    }
    out
}

/// The point nearest `at`, for the crosshair.
pub fn nearest<'a>(points: impl Iterator<Item = &'a Point>, at: u64) -> Option<&'a Point> {
    points.min_by_key(|p| p.at.abs_diff(at))
}

/// A percent as CHROTE writes it: one decimal below 10, whole above,
/// `--` when unknown.
pub fn percent(v: Option<f32>) -> String {
    match v {
        None => "--".into(),
        Some(v) => {
            let v = v.clamp(0., 100.);
            if v < 10. {
                format!("{v:.1}%")
            } else {
                format!("{v:.0}%")
            }
        }
    }
}

/// The line Send hands a host's reading over with: where, and what it read
/// last, in the words the Fleet page shows.
pub fn reference(device: &str, environment: &str, r: &Reading) -> String {
    let mut parts = vec![format!("CPU {}", percent(r.cpu))];
    if let Some(m) = r.memory {
        parts.push(format!("memory {}", usage(m)));
    }
    if let Some(d) = &r.disk {
        let used = Usage {
            used: d.used,
            total: d.total,
        };
        parts.push(format!("disk {} {}", d.mount, usage(used)));
    }
    if let Some([one, ..]) = r.load {
        parts.push(format!("load {one:.2}"));
    }
    for g in &r.gpus {
        parts.push(format!("{} {}", g.name, percent(g.util)));
    }
    if let Some(up) = r.uptime {
        parts.push(format!("up {}", uptime(up)));
    }
    format!("host {device} ({environment}): {}", parts.join(", "))
}

/// Bytes in 1024s: B, KB, MB, GB, TB; one decimal below 10.
pub fn bytes(n: u64) -> String {
    if n == 0 {
        return "0 B".into();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024. && i < units.len() - 1 {
        v /= 1024.;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else if v < 10. {
        format!("{v:.1} {}", units[i])
    } else {
        format!("{v:.0} {}", units[i])
    }
}

pub fn usage(u: Usage) -> String {
    format!("{} / {}", bytes(u.used), bytes(u.total))
}

/// `3d 4h`, `4h 12m`, `12m`.
pub fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_host_contract::Disk;

    #[test]
    fn a_reading_is_handed_over_as_the_page_words_it() {
        let r = Reading {
            cpu: Some(12.4),
            memory: Some(Usage {
                used: 4 << 30,
                total: 16 << 30,
            }),
            disk: Some(Disk {
                mount: "/".into(),
                used: 100 << 30,
                total: 400 << 30,
            }),
            load: Some([0.52, 0.4, 0.3]),
            uptime: Some(3 * 86_400 + 4 * 3600),
            ..Default::default()
        };
        assert_eq!(
            reference("rig", "wsl", &r),
            "host rig (wsl): CPU 12%, memory 4.0 GB / 16 GB, disk / 100 GB / 400 GB, load 0.52, up 3d 4h"
        );
        assert_eq!(
            reference("pad", "windows", &Reading::default()),
            "host pad (windows): CPU --"
        );
    }

    fn p(at: u64, cpu: Option<f32>) -> Point {
        Point {
            at,
            cpu,
            ..Point::default()
        }
    }

    #[test]
    fn scales_each_row_to_its_own_peak() {
        // CHROTE: GPU at 17% gets a 25% ceiling, CPU 30% a 40% one.
        assert_eq!(ceiling([5., 17., 12.].into_iter()), 25.);
        assert_eq!(ceiling([30.].into_iter()), 40.);
        assert_eq!(ceiling([13.].into_iter()), 20.);
    }

    #[test]
    fn floors_an_all_zero_row_and_caps_a_saturated_one() {
        assert_eq!(ceiling([0., 0.].into_iter()), 5.);
        assert_eq!(ceiling([88., 94.].into_iter()), 100.);
        assert_eq!(ceiling(std::iter::empty()), 100.);
    }

    #[test]
    fn window_stats_are_peak_and_mean() {
        assert_eq!(stats([7., 17., 12.].into_iter()), Some((17., 12.)));
        assert_eq!(stats(std::iter::empty()), None);
    }

    #[test]
    fn a_missing_figure_or_a_silence_breaks_the_line() {
        let points = [
            p(0, Some(1.)),
            p(5000, Some(2.)),
            p(10000, None),
            p(15000, Some(3.)),
            p(20000, Some(4.)),
            // The node was away for a minute.
            p(80000, Some(5.)),
        ];
        let r = runs(&points, Metric::Cpu, 5000);
        let lens: Vec<usize> = r.iter().map(Vec::len).collect();
        assert_eq!(lens, vec![2, 2, 1]);
    }

    fn reading(at: u64, cpu: f32) -> Vec<u8> {
        format!(
            r#"{{"reading":{{"at":{at},"cpu":{cpu},"memory":{{"used":1,"total":2}},
            "swap":{{"used":1,"total":8}},"load":[0.5,0.4,0.3],
            "disk":{{"mount":"/","used":1,"total":4}},"gpus":[],"uptime":60}},
            "point":[{at},{cpu},50.0,null,3.1,12.5,null]}}"#
        )
        .into_bytes()
    }

    #[test]
    fn readings_extend_the_history_and_fold_what_is_older_than_an_hour() {
        let mut h = Hosts::default();
        let state =
            br#"{"facts":{"os":"Ubuntu","kernel":null,"cpu":"Ryzen","user":"u","threads":16},
            "every":5000,"bucket":60000,"earlier":[[0,9.0,2.0,null,null,null,null]],
            "history":[[61000,1.0,2.0,null,null,null,null],[62000,3.0,4.0,null,null,null,null]],
            "latest":null}"#;
        assert!(h.take("n", STATE, state).is_some());
        assert!(h.take("n", READING, &reading(3_661_500, 5.)).is_some());
        let s = &h.nodes["n"];
        assert_eq!(s.history.len(), 2, "61000 is older than an hour");
        assert_eq!(s.history[1].load, Some(3.1));
        assert_eq!(
            s.latest.as_ref().and_then(|r| r.load),
            Some([0.5, 0.4, 0.3])
        );
        // 61000 waits in its minute's bucket until the next minute starts.
        assert_eq!(s.earlier.len(), 1);
        // 62000 joins it; 3661500 leaving an hour later starts the next.
        assert!(h.take("n", READING, &reading(7_261_500, 7.)).is_some());
        let s = &h.nodes["n"];
        assert_eq!(s.earlier.len(), 2);
        assert_eq!((s.earlier[1].at, s.earlier[1].cpu), (60_000, Some(2.0)));
        assert!(h.take("n", 0x88, b"{}").is_none());
    }

    #[test]
    fn a_day_of_readings_keeps_a_day_in_minutes() {
        let mut h = Hosts::default();
        let state =
            br#"{"facts":{},"every":5000,"bucket":60000,"earlier":[],"history":[],"latest":null}"#;
        h.take("n", STATE, state);
        let every = 5_000;
        for i in 1..=(26 * 3600 * 1000 / every) {
            h.take("n", READING, &reading(i * every, (i % 10) as f32));
        }
        let s = &h.nodes["n"];
        assert_eq!(s.history.len(), 720);
        let newest = s.history.last().unwrap().at;
        assert!(newest - s.oldest().unwrap() <= DAY);
        assert!(s.earlier.len() >= 23 * 60 - 1 && s.earlier.len() <= 23 * 60);
        assert!(s.earlier.windows(2).all(|w| w[1].at - w[0].at == 60_000));
    }

    #[test]
    fn the_line_runs_from_the_buckets_into_the_readings() {
        let state = State {
            every: 5000,
            bucket: 60_000,
            earlier: vec![p(0, Some(1.)), p(60_000, Some(2.)), p(120_000, Some(3.))],
            history: vec![p(185_000, Some(4.)), p(190_000, Some(5.))],
            ..State::default()
        };
        let lens: Vec<usize> = state.runs(Metric::Cpu, 0).iter().map(Vec::len).collect();
        assert_eq!(lens, vec![5]);
        let lens: Vec<usize> = state
            .runs(Metric::Cpu, 60_000)
            .iter()
            .map(Vec::len)
            .collect();
        assert_eq!(lens, vec![4], "the window starts at the second bucket");
        let young = State {
            earlier: vec![],
            ..state.clone()
        };
        let lens: Vec<usize> = young.runs(Metric::Cpu, 0).iter().map(Vec::len).collect();
        assert_eq!(
            lens,
            vec![2],
            "a node up less than an hour has only readings"
        );
        let away = State {
            history: vec![p(900_000, Some(4.))],
            ..state
        };
        let lens: Vec<usize> = away.runs(Metric::Cpu, 0).iter().map(Vec::len).collect();
        assert_eq!(lens, vec![3, 1], "ten minutes away is a gap");
    }

    #[test]
    fn missing_figures_at_the_history_boundary_are_not_bridged() {
        let state = State {
            every: 5_000,
            bucket: 60_000,
            earlier: vec![p(0, Some(1.)), p(60_000, None)],
            history: vec![p(120_000, Some(2.))],
            ..State::default()
        };
        assert_eq!(state.runs(Metric::Cpu, 0).len(), 2);

        let state = State {
            earlier: vec![p(0, Some(1.))],
            history: vec![p(60_000, None), p(65_000, Some(2.))],
            ..state
        };
        assert_eq!(state.runs(Metric::Cpu, 0).len(), 2);
    }

    #[test]
    fn a_missing_minute_bucket_is_not_bridged() {
        let state = State {
            bucket: 60_000,
            earlier: vec![p(0, Some(1.)), p(120_000, Some(2.))],
            ..State::default()
        };
        assert_eq!(state.runs(Metric::Cpu, 0).len(), 2);
    }

    #[test]
    fn the_charts_span_what_was_kept_between_an_hour_and_a_day() {
        assert_eq!(span(None, 5 * HOUR), HOUR);
        assert_eq!(span(Some(4 * HOUR + 1), 5 * HOUR), HOUR);
        assert_eq!(span(Some(2 * HOUR), 5 * HOUR), 3 * HOUR);
        assert_eq!(span(Some(0), 30 * HOUR), DAY);
    }

    #[test]
    fn the_axis_ticks_whole_local_hours_and_leaves_the_now_edge_clear() {
        let at = |h: u64, m: u64| h * HOUR + m * 60_000;
        // A day, in UTC: every four hours, the last one two hours clear of now.
        assert_eq!(
            ticks(at(0, 20), at(24, 20), 0),
            vec![at(4, 0), at(8, 0), at(12, 0), at(16, 0), at(20, 0)]
        );
        // An hour: every ten minutes, none within five minutes of now.
        assert_eq!(
            ticks(at(10, 3), at(11, 3), 0),
            vec![at(10, 10), at(10, 20), at(10, 30), at(10, 40), at(10, 50)]
        );
        // Helsinki in winter is two hours ahead: local 00:00 is 22:00 UTC.
        let helsinki = ticks(at(20, 0), at(32, 0), 7200);
        assert_eq!(helsinki[0], at(22, 0));
        assert!(helsinki.iter().all(|t| (t / 1000 + 7200) % 7200 == 0));
    }

    #[test]
    fn events_come_with_the_state_and_one_by_one_and_stay_bounded() {
        let mut h = Hosts::default();
        let state =
            br#"{"facts":{},"every":5000,"bucket":60000,"earlier":[],"history":[],"latest":null,
            "events":[{"at":1,"kind":"node","failure":false,"text":"Parvi 0.0.1 started"}]}"#;
        assert_eq!(h.take("n", STATE, state), Some(None));
        for i in 0..EVENTS_KEPT {
            let e = format!(
                r#"{{"event":{{"at":{},"kind":"peer","failure":true,"text":"rig went offline"}}}}"#,
                i + 2
            );
            let got = h.take("n", EVENT, e.as_bytes()).unwrap().unwrap();
            assert!(got.failure);
        }
        let events = &h.nodes["n"].events;
        assert_eq!(events.len(), EVENTS_KEPT);
        assert_eq!(events.last().unwrap().at, EVENTS_KEPT as u64 + 1);
        assert!(events.iter().all(|e| e.kind == "peer"), "the oldest went");
        assert_eq!(h.take("n", 0x40, b"{}"), None, "not Host's");
    }

    #[test]
    fn writes_figures_as_chrote_does() {
        assert_eq!(percent(Some(4.25)), "4.2%");
        assert_eq!(percent(Some(30.4)), "30%");
        assert_eq!(percent(Some(130.)), "100%");
        assert_eq!(percent(None), "--");
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(8 * 1024 * 1024 * 1024 - 100 * 1024 * 1024), "7.9 GB");
        assert_eq!(bytes(280 * 1024u64.pow(3)), "280 GB");
        assert_eq!(uptime(277_200), "3d 5h");
        assert_eq!(uptime(4 * 3600 + 12 * 60), "4h 12m");
        assert_eq!(uptime(600), "10m");
    }
}
