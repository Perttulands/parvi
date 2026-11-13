//! Each node's host as its host channel tells it, and what the page makes
//! of it: the figures each row reads, what is wrong now, and the words a
//! reading is written and handed over in. Plain data; the page draws it.

pub use parvi_host_contract::{Disk, Event, Heavy, Point, Reading, State, Usage};
use parvi_host_contract::{
    EventFrame, HOST_EVENT as EVENT, HOST_READING as READING, HOST_STATE as STATE, MINUTE, Minute,
    ReadingFrame,
};
use std::collections::HashMap;

/// Events the app keeps per node: the node keeps more.
pub const EVENTS_KEPT: usize = 50;
/// An hour, in ms: how far back a row's small line reaches.
pub const HOUR: u64 = 3_600_000;
/// A day: how far back the history reaches.
pub const DAY: u64 = 24 * HOUR;

/// One node's host: the state as the channel opened, the minute filling
/// since, and the minutes closed since.
#[derive(Default)]
pub struct NodeHost {
    pub state: State,
    open: Option<Minute>,
}

/// What every node's host channel said last. A node that goes away keeps
/// its figures, so the page shows what it last knew.
#[derive(Default)]
pub struct Hosts {
    pub nodes: HashMap<String, NodeHost>,
}

impl Hosts {
    /// Takes a host frame from `node`, giving back a new event, which the
    /// status line tells. `None` if the frame was not Host's.
    pub fn take(&mut self, node: &str, kind: u8, body: &[u8]) -> Option<Option<Event>> {
        match kind {
            EVENT => {
                let e = serde_json::from_slice::<EventFrame>(body)
                    .ok()
                    .map(|f| f.event);
                if let (Some(e), Some(n)) = (&e, self.nodes.get_mut(node)) {
                    n.state.events.push(e.clone());
                    let over = n.state.events.len().saturating_sub(EVENTS_KEPT);
                    n.state.events.drain(..over);
                }
                Some(e)
            }
            STATE => {
                if let Ok(mut s) = serde_json::from_slice::<State>(body) {
                    let over = s.events.len().saturating_sub(EVENTS_KEPT);
                    s.events.drain(..over);
                    self.nodes.insert(
                        node.to_string(),
                        NodeHost {
                            state: s,
                            open: None,
                        },
                    );
                }
                Some(None)
            }
            READING => {
                if let Ok(r) = serde_json::from_slice::<ReadingFrame>(body)
                    && let Some(n) = self.nodes.get_mut(node)
                {
                    n.add(r);
                }
                Some(None)
            }
            _ => None,
        }
    }
}

impl NodeHost {
    /// A reading joins its minute, as the node's history does: the last
    /// minute the state brought, if it is still this one, or a new one.
    fn add(&mut self, r: ReadingFrame) {
        let minutes = &mut self.state.minutes;
        let open = match self.open.take() {
            Some(m) if m.holds(r.point.at) => Some(m),
            Some(m) => {
                if let Some(last) = minutes.last_mut()
                    && m.holds(last.at)
                {
                    *last = m.point();
                }
                None
            }
            None => None,
        };
        let mut minute = open.unwrap_or_else(|| {
            let mut m = Minute::starting(r.point.at);
            // The state's last minute was still filling: start from it.
            if let Some(last) = minutes.last()
                && m.holds(last.at)
            {
                m.add(last);
            }
            m
        });
        minute.add(&r.point);
        match minutes.last_mut() {
            Some(last) if minute.holds(last.at) => *last = minute.point(),
            _ => minutes.push(minute.point()),
        }
        let end = r.point.at;
        minutes.retain(|p| p.at + DAY + MINUTE > end);
        self.open = Some(minute);
        self.state.latest = Some(r.reading);
    }
}

/// What a row of the page reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Figure {
    Cpu,
    Memory,
    Gpu,
    Vram,
    /// The reading's disk at this place.
    Disk(usize),
}

impl Figure {
    pub fn of(self, p: &Point) -> Option<f32> {
        match self {
            Figure::Cpu => p.cpu,
            Figure::Memory => p.memory,
            Figure::Gpu => p.gpu,
            Figure::Vram => p.vram,
            Figure::Disk(i) => p.disks.get(i).copied().flatten(),
        }
    }

    /// Its name, as a row says it.
    pub fn name(self, r: Option<&Reading>) -> String {
        match self {
            Figure::Cpu => "CPU".into(),
            Figure::Memory => "Memory".into(),
            Figure::Gpu => "GPU".into(),
            Figure::Vram => "GPU memory".into(),
            Figure::Disk(i) => {
                let name = r
                    .and_then(|r| r.disks.get(i))
                    .map_or("", |d| d.name.as_str());
                format!("Disk {name}")
            }
        }
    }

    /// Its value now, in percent.
    pub fn now(self, r: &Reading) -> Option<f32> {
        match self {
            Figure::Cpu => r.cpu,
            Figure::Memory => r.memory.and_then(Usage::percent),
            Figure::Gpu => r.gpus.first().and_then(|g| g.util),
            Figure::Vram => r.gpus.first().and_then(|g| g.memory?.percent()),
            Figure::Disk(i) => r.disks.get(i).and_then(|d| d.usage().percent()),
        }
    }

    /// Whether it fills up (and is drawn against the whole) rather than
    /// being busy.
    pub fn fills(self) -> bool {
        !matches!(self, Figure::Cpu | Figure::Gpu)
    }
}

/// The rows a node's host has: the GPU's only where a GPU is or was read.
pub fn figures(r: Option<&Reading>) -> Vec<Figure> {
    let mut out = vec![Figure::Cpu, Figure::Memory];
    let gpu = r.is_some_and(|r| {
        !r.gpus.is_empty()
            || r.gpu_reason
                .as_deref()
                .is_some_and(|why| !why.contains("not found"))
    });
    if gpu {
        out.extend([Figure::Gpu, Figure::Vram]);
    }
    let disks = r.map_or(0, |r| r.disks.len());
    out.extend((0..disks).map(Figure::Disk));
    out
}

/// The figure's points from `start` on, as (at, value).
pub fn line(minutes: &[Point], figure: Figure, start: u64) -> Vec<(u64, f32)> {
    minutes
        .iter()
        .filter(|p| p.at >= start)
        .filter_map(|p| Some((p.at, figure.of(p)?)))
        .collect()
}

/// The line as runs: a missing minute breaks it, since nothing was read
/// there (the node was down).
pub fn runs(points: &[(u64, f32)]) -> Vec<Vec<(u64, f32)>> {
    let mut out: Vec<Vec<(u64, f32)>> = vec![];
    for &(at, v) in points {
        match out.last_mut() {
            Some(run)
                if run
                    .last()
                    .is_some_and(|(l, _)| at.abs_diff(*l) <= 2 * MINUTE) =>
            {
                run.push((at, v))
            }
            _ => out.push(vec![(at, v)]),
        }
    }
    out
}

/// The highest point of `points` and when.
pub fn peak(points: &[(u64, f32)]) -> Option<(u64, f32)> {
    points
        .iter()
        .copied()
        .fold(None, |best, (at, v)| match best {
            Some((_, b)) if b >= v => best,
            _ => Some((at, v)),
        })
}

/// A chart's top: the whole for what fills up; for what is busy, its peak
/// and a quarter to the next 5%, between 5 and 100, so a quiet day still
/// shows its shape.
pub fn ceiling(figure: Figure, points: &[(u64, f32)]) -> f32 {
    if figure.fills() {
        return 100.;
    }
    let peak = points.iter().fold(0f32, |m, (_, v)| m.max(*v));
    ((peak * 1.25 / 5.).ceil() * 5.).clamp(5., 100.)
}

/// When a figure needs the operator: memory nearly full, a disk nearly
/// full or nearly out of room, the GPU's memory full or the card hot.
const FULL_MEMORY: f32 = 90.;
const FULL_DISK: f32 = 90.;
const LOW_DISK: u64 = 10 << 30;
const FULL_VRAM: f32 = 95.;
const HOT_GPU: f32 = 85.;
/// Swapping this fast means memory is short.
const SWAPPING: f32 = 1024. * 1024.;

/// Something wrong with the host now, and the row that shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Trouble {
    pub figure: Option<Figure>,
    pub text: String,
}

/// What is wrong with the host now, most pressing first. Empty when all is
/// well. `now` is Unix ms, for how recent an out-of-memory kill was.
pub fn troubles(s: &State, now: u64) -> Vec<Trouble> {
    let mut out = vec![];
    let Some(r) = s.latest.as_ref() else {
        return out;
    };
    if let Some(m) = r.memory
        && m.percent().is_some_and(|p| p >= FULL_MEMORY)
    {
        out.push(Trouble {
            figure: Some(Figure::Memory),
            text: format!("Memory {} full: {}", percent(m.percent()), of(m)),
        });
    }
    if let Some(bps) = r.swap_activity_bps
        && bps >= SWAPPING
    {
        out.push(Trouble {
            figure: Some(Figure::Memory),
            text: format!("Swapping {}/s: memory is short", bytes(bps as u64)),
        });
    }
    for e in s.events.iter().rev() {
        if e.kind == "host" && e.failure && e.at + DAY > now {
            out.push(Trouble {
                figure: Some(Figure::Memory),
                text: format!("{} at {}", e.text, crate::readings::hhmm(e.at)),
            });
            break;
        }
    }
    for (i, d) in r.disks.iter().enumerate() {
        let free = d.total.saturating_sub(d.used);
        let share = d.usage().percent();
        if share.is_some_and(|p| p >= FULL_DISK) || (d.total > 0 && free < LOW_DISK) {
            out.push(Trouble {
                figure: Some(Figure::Disk(i)),
                text: format!(
                    "Disk {} {} full: {} free",
                    d.name,
                    percent(share),
                    bytes(free)
                ),
            });
        }
    }
    if let Some(g) = r.gpus.first() {
        if let Some(m) = g.memory
            && m.percent().is_some_and(|p| p >= FULL_VRAM)
        {
            out.push(Trouble {
                figure: Some(Figure::Vram),
                text: format!("GPU memory {} full: {}", percent(m.percent()), of(m)),
            });
        }
        if let Some(c) = g.temp
            && c >= HOT_GPU
        {
            out.push(Trouble {
                figure: Some(Figure::Gpu),
                text: format!("GPU at {c:.0} °C"),
            });
        }
    }
    out
}

/// Whether a figure's row is in trouble now.
pub fn warns(troubles: &[Trouble], figure: Figure) -> bool {
    troubles.iter().any(|t| t.figure == Some(figure))
}

/// A percent: one decimal below 10, whole above, `--` when unknown.
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

/// `2.9 GB of 31 GB`.
pub fn of(u: Usage) -> String {
    format!("{} of {}", bytes(u.used), bytes(u.total))
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

/// A folder as a row has room for: home as `~`.
pub fn folder(cwd: &str, home: Option<&str>) -> String {
    match home {
        Some(h) if !h.is_empty() && cwd.starts_with(h) => format!("~{}", &cwd[h.len()..]),
        _ => cwd.to_owned(),
    }
}

/// Where a heavy program runs, as its line says it: the session working in
/// its folder, if one is (the deepest, but never one at `/` or home, which
/// would claim everything), then the folder.
pub fn place(cwd: Option<&str>, sessions: &[(&str, &str)], home: Option<&str>) -> String {
    let Some(cwd) = cwd else {
        return String::new();
    };
    let session = sessions
        .iter()
        .filter(|(_, at)| *at != "/" && Some(*at) != home)
        .filter(|(_, at)| {
            cwd == *at
                || cwd
                    .strip_prefix(at)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(|(_, at)| at.len());
    let folder = folder(cwd, home);
    match session {
        Some((name, _)) => format!("{name} · {folder}"),
        None => folder,
    }
}

/// A heavy program as a line says it: `rustc ×12`.
pub fn program(h: &Heavy) -> String {
    match h.pids.len() {
        0 | 1 => h.name.clone(),
        n => format!("{} ×{n}", h.name),
    }
}

/// What one figure reads now, in words: `2.9 GB of 31 GB (9.4%)`.
pub fn reading_words(figure: Figure, r: &Reading) -> String {
    let p = percent(figure.now(r));
    let used = match figure {
        Figure::Memory => r.memory,
        Figure::Vram => r.gpus.first().and_then(|g| g.memory),
        Figure::Disk(i) => r.disks.get(i).map(Disk::usage),
        Figure::Cpu | Figure::Gpu => None,
    };
    match used {
        Some(u) => format!("{} ({p})", of(u)),
        None => p,
    }
}

/// The line Send hands the host over with: where, what is wrong or that
/// all is well, every figure now and what is heaviest.
pub fn reference(device: &str, environment: &str, s: &State, now: u64) -> String {
    let Some(r) = s.latest.as_ref() else {
        return format!("host {device} ({environment}): no reading yet");
    };
    let troubles = troubles(s, now);
    let verdict = if troubles.is_empty() {
        "all right".to_owned()
    } else {
        troubles
            .iter()
            .map(|t| t.text.clone())
            .collect::<Vec<_>>()
            .join("; ")
    };
    let figures: Vec<String> = figures(Some(r))
        .into_iter()
        .map(|f| format!("{} {}", f.name(Some(r)), reading_words(f, r)))
        .collect();
    let mut out = format!(
        "host {device} ({environment}): {verdict}. {}",
        figures.join(", ")
    );
    if let Some(up) = r.uptime {
        out.push_str(&format!(", up {}", uptime(up)));
    }
    let heavy: Vec<String> = r.heavy.iter().take(3).map(heavy_words).collect();
    if !heavy.is_empty() {
        out.push_str(&format!(". Heaviest: {}", heavy.join("; ")));
    }
    out
}

/// One figure handed over: now, and its highest today.
pub fn figure_reference(
    device: &str,
    environment: &str,
    s: &State,
    figure: Figure,
    start: u64,
) -> String {
    let r = s.latest.as_ref();
    let name = figure.name(r).to_lowercase();
    let now = r.map_or_else(|| "no reading".into(), |r| reading_words(figure, r));
    let mut out = format!("host {device} ({environment}) {name}: {now}");
    if let Some((at, v)) = peak(&line(&s.minutes, figure, start)) {
        out.push_str(&format!(
            ", highest {} at {}",
            percent(Some(v)),
            crate::readings::hhmm(at)
        ));
    }
    if matches!(figure, Figure::Cpu | Figure::Memory)
        && let Some(r) = r
    {
        let heavy: Vec<String> = sorted(&r.heavy, figure)
            .into_iter()
            .take(3)
            .map(heavy_words)
            .collect();
        if !heavy.is_empty() {
            out.push_str(&format!(". Heaviest: {}", heavy.join("; ")));
        }
    }
    out
}

fn heavy_words(h: &Heavy) -> String {
    let mut out = program(h);
    if let Some(cwd) = &h.cwd {
        out.push_str(&format!(" in {cwd}"));
    }
    out.push_str(&format!(" {}", bytes(h.memory)));
    if let Some(cpu) = h.cpu {
        out.push_str(&format!(" {} CPU", percent(Some(cpu))));
    }
    out
}

/// What is heavy, heaviest by `figure` first (memory unless CPU).
pub fn sorted(heavy: &[Heavy], figure: Figure) -> Vec<&Heavy> {
    let mut out: Vec<&Heavy> = heavy.iter().collect();
    if figure == Figure::Cpu {
        out.sort_by(|a, b| b.cpu.unwrap_or(0.).total_cmp(&a.cpu.unwrap_or(0.)));
    } else {
        out.sort_by_key(|h| std::cmp::Reverse(h.memory));
    }
    out
}

/// How much a disk's use changed since `start`: bytes (may be negative),
/// from its share then and its size now.
pub fn grew(s: &State, disk: usize, start: u64) -> Option<(i64, u64)> {
    let d = s.latest.as_ref()?.disks.get(disk)?;
    let (at, then) = line(&s.minutes, Figure::Disk(disk), start)
        .first()
        .copied()?;
    let before = (then as f64 / 100. * d.total as f64) as i64;
    Some((d.used as i64 - before, at))
}

/// A disk's change in words. The history keeps a tenth of a percent, so
/// less than that of the disk is no change it can tell.
pub fn change(grew: i64, total: u64) -> String {
    if grew.unsigned_abs() <= total / 1000 {
        return "unchanged".into();
    }
    let sign = if grew < 0 { "−" } else { "+" };
    format!("{sign}{}", bytes(grew.unsigned_abs()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_host_contract::Gpu;

    fn p(at: u64, cpu: f32) -> Point {
        Point {
            at,
            cpu: Some(cpu),
            ..Point::default()
        }
    }

    fn frame(at: u64, cpu: f32) -> Vec<u8> {
        let reading = Reading {
            at,
            cpu: Some(cpu),
            ..Reading::default()
        };
        serde_json::to_vec(&ReadingFrame {
            point: Point::of(&reading),
            reading,
        })
        .unwrap()
    }

    #[test]
    fn readings_fill_the_minute_the_state_left_open_then_the_next() {
        let mut h = Hosts::default();
        let state = State {
            minutes: vec![p(0, 1.), p(MINUTE, 10.)],
            ..State::default()
        };
        h.take("n", STATE, &serde_json::to_vec(&state).unwrap());
        h.take("n", READING, &frame(MINUTE + 30_000, 30.));
        let minutes = &h.nodes["n"].state.minutes;
        assert_eq!(minutes.len(), 2);
        assert_eq!(minutes[1].cpu, Some(20.), "the open minute and the reading");
        h.take("n", READING, &frame(2 * MINUTE + 1, 50.));
        h.take("n", READING, &frame(2 * MINUTE + 2, 70.));
        let n = &h.nodes["n"].state;
        assert_eq!(n.minutes.len(), 3);
        assert_eq!(n.minutes[1].cpu, Some(20.), "the closed minute stays");
        assert_eq!(n.minutes[2].cpu, Some(60.));
        assert_eq!(n.latest.as_ref().unwrap().cpu, Some(70.));
        assert!(h.take("n", 0x88, b"{}").is_none());
    }

    #[test]
    fn a_day_of_readings_keeps_a_day() {
        let mut h = Hosts::default();
        h.take("n", STATE, &serde_json::to_vec(&State::default()).unwrap());
        for i in 0..(26 * 60) {
            h.take("n", READING, &frame(i * MINUTE, 1.));
        }
        let minutes = &h.nodes["n"].state.minutes;
        assert_eq!(minutes.len(), 24 * 60 + 1);
    }

    #[test]
    fn events_come_with_the_state_and_one_by_one_and_stay_bounded() {
        let mut h = Hosts::default();
        let state = br#"{"facts":{},"minutes":[],"latest":null,
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
        let events = &h.nodes["n"].state.events;
        assert_eq!(events.len(), EVENTS_KEPT);
        assert!(events.iter().all(|e| e.kind == "peer"), "the oldest went");
    }

    #[test]
    fn a_missing_minute_breaks_the_line() {
        let points = [(0, 1.), (MINUTE, 2.), (5 * MINUTE, 3.), (6 * MINUTE, 4.)];
        let lens: Vec<usize> = runs(&points).iter().map(Vec::len).collect();
        assert_eq!(lens, [2, 2]);
    }

    fn disk(name: &str, used: u64, total: u64) -> Disk {
        Disk {
            name: name.into(),
            mount: name.into(),
            used,
            total,
        }
    }

    #[test]
    fn all_is_well_says_nothing_and_each_trouble_names_its_row() {
        let gib = 1u64 << 30;
        let mut s = State {
            latest: Some(Reading {
                memory: Some(Usage {
                    used: 4 * gib,
                    total: 32 * gib,
                }),
                disks: vec![
                    disk("/", 400 * gib, 800 * gib),
                    disk("C:", 1900 * gib, 1905 * gib),
                ],
                gpus: vec![Gpu {
                    memory: Some(Usage { used: 2, total: 16 }),
                    temp: Some(40.),
                    ..Gpu::default()
                }],
                ..Reading::default()
            }),
            ..State::default()
        };
        let t = troubles(&s, 0);
        assert_eq!(t.len(), 1, "{t:?}");
        assert_eq!(t[0].figure, Some(Figure::Disk(1)));
        assert_eq!(t[0].text, "Disk C: 100% full: 5.0 GB free");
        s.latest.as_mut().unwrap().disks.pop();
        assert!(troubles(&s, 0).is_empty());

        let r = s.latest.as_mut().unwrap();
        r.memory = Some(Usage {
            used: 30 * gib,
            total: 32 * gib,
        });
        r.swap_activity_bps = Some(4. * 1024. * 1024.);
        s.events.push(Event {
            at: 5,
            kind: "host".into(),
            failure: true,
            text: "Out of memory: the kernel killed a process (1 since boot)".into(),
        });
        let t = troubles(&s, 10);
        assert_eq!(t.len(), 3, "{t:?}");
        assert!(t.iter().all(|t| t.figure == Some(Figure::Memory)));
        assert!(
            troubles(&s, 5 + DAY).len() == 2,
            "a day-old kill is history"
        );
    }

    #[test]
    fn rows_show_a_gpu_only_where_one_is_read() {
        let mut r = Reading {
            gpu_reason: Some("nvidia-smi not found".into()),
            disks: vec![disk("/", 1, 2)],
            ..Reading::default()
        };
        assert_eq!(
            figures(Some(&r)),
            [Figure::Cpu, Figure::Memory, Figure::Disk(0)]
        );
        r.gpu_reason = Some("nvidia-smi exit status: 9".into());
        assert!(
            figures(Some(&r)).contains(&Figure::Vram),
            "a GPU that failed"
        );
    }

    #[test]
    fn a_reading_is_handed_over_as_the_page_words_it() {
        let gib = 1u64 << 30;
        let s = State {
            latest: Some(Reading {
                cpu: Some(12.4),
                memory: Some(Usage {
                    used: 4 * gib,
                    total: 16 * gib,
                }),
                disks: vec![disk("/", 100 * gib, 400 * gib)],
                uptime: Some(3 * 86_400 + 4 * 3600),
                heavy: vec![Heavy {
                    name: "rustc".into(),
                    cwd: Some("/srv/x".into()),
                    pids: vec![1, 2],
                    cpu: Some(40.),
                    memory: 2 * gib,
                }],
                ..Reading::default()
            }),
            ..State::default()
        };
        assert_eq!(
            reference("rig", "wsl", &s, 0),
            "host rig (wsl): all right. CPU 12%, Memory 4.0 GB of 16 GB (25%), \
             Disk / 100 GB of 400 GB (25%), up 3d 4h. Heaviest: rustc ×2 in /srv/x 2.0 GB 40% CPU"
        );
        assert_eq!(
            reference("pad", "windows", &State::default(), 0),
            "host pad (windows): no reading yet"
        );
    }

    #[test]
    fn a_disk_says_how_much_it_grew() {
        let gib = 1u64 << 30;
        let s = State {
            minutes: vec![Point {
                at: 7,
                disks: vec![Some(50.)],
                ..Point::default()
            }],
            latest: Some(Reading {
                disks: vec![disk("/", 60 * gib, 100 * gib)],
                ..Reading::default()
            }),
            ..State::default()
        };
        assert_eq!(grew(&s, 0, 0), Some(((10 * gib) as i64, 7)));
        assert_eq!(grew(&s, 1, 0), None);
        let tb = 1u64 << 40;
        assert_eq!(change(-(129 << 20), 2 * tb), "unchanged", "within rounding");
        assert_eq!(change(40 << 30, 2 * tb), "+40 GB");
        assert_eq!(change(-(5 << 30), 2 * tb), "−5.0 GB");
    }

    #[test]
    fn busy_rows_scale_to_their_peak_and_full_ones_to_the_whole() {
        assert_eq!(ceiling(Figure::Gpu, &[(0, 5.), (1, 17.)]), 25.);
        assert_eq!(ceiling(Figure::Cpu, &[(0, 0.)]), 5.);
        assert_eq!(ceiling(Figure::Memory, &[(0, 9.)]), 100.);
    }

    #[test]
    fn the_axis_ticks_whole_local_hours_and_leaves_the_now_edge_clear() {
        let at = |h: u64, m: u64| h * HOUR + m * 60_000;
        assert_eq!(
            ticks(at(0, 20), at(24, 20), 0),
            vec![at(4, 0), at(8, 0), at(12, 0), at(16, 0), at(20, 0)]
        );
        let helsinki = ticks(at(20, 0), at(32, 0), 7200);
        assert_eq!(helsinki[0], at(22, 0));
    }

    #[test]
    fn writes_figures_plainly() {
        assert_eq!(percent(Some(4.25)), "4.2%");
        assert_eq!(percent(Some(30.4)), "30%");
        assert_eq!(percent(None), "--");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(280 * 1024u64.pow(3)), "280 GB");
        assert_eq!(uptime(4 * 3600 + 12 * 60), "4h 12m");
        assert_eq!(folder("/home/me/x", Some("/home/me")), "~/x");
        assert_eq!(folder("/srv/x", Some("/home/me")), "/srv/x");
    }

    #[test]
    fn a_heavy_program_is_placed_in_the_session_working_in_its_folder() {
        let home = Some("/home/me");
        let sessions = [
            ("shell", "/home/me"),
            ("root", "/"),
            ("parvi", "/srv/parvi"),
            ("ui", "/srv/parvi/ui"),
        ];
        let at = |cwd| place(Some(cwd), &sessions, home);
        assert_eq!(at("/srv/parvi/ui/src"), "ui · /srv/parvi/ui/src");
        assert_eq!(at("/srv/parvi"), "parvi · /srv/parvi");
        assert_eq!(at("/srv/parvi-next"), "/srv/parvi-next", "not a prefix");
        assert_eq!(at("/home/me/x"), "~/x", "home claims nothing");
        assert_eq!(place(None, &sessions, home), "");
    }
}
