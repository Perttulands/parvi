//! Host and service wire types shared by the node and app.
use serde::{Deserialize, Serialize};

pub const HOST: u8 = 0x30;
pub const HOST_STATE: u8 = 0x31;
pub const HOST_READING: u8 = 0x32;
pub const HOST_EVENT: u8 = 0x33;
pub const BAD_REQUEST: &str = "bad_request";
pub const NOT_FOUND: &str = "not_found";
pub const UNAVAILABLE: &str = "unavailable";
pub const SERVICE_FAILED: &str = "service_failed";

pub fn encode_on(on: bool) -> Vec<u8> {
    vec![HOST, u8::from(on)]
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub at: u64,
    pub kind: String,
    #[serde(default)]
    pub failure: bool,
    pub text: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Facts {
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub cpu: Option<String>,
    pub user: Option<String>,
    #[serde(default)]
    pub threads: usize,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub used: u64,
    pub total: u64,
}
impl Usage {
    pub fn percent(self) -> Option<f32> {
        (self.total > 0).then(|| self.used as f32 / self.total as f32 * 100.0)
    }
}
/// A file system the operator can fill: the root, and on WSL each Windows
/// drive it mounts (`C:` at `/mnt/c`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Disk {
    /// What the operator calls it: `/`, `C:`.
    pub name: String,
    /// Where it is mounted here.
    pub mount: String,
    pub used: u64,
    pub total: u64,
}
impl Disk {
    pub fn usage(&self) -> Usage {
        Usage {
            used: self.used,
            total: self.total,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub util: Option<f32>,
    pub memory: Option<Usage>,
    pub temp: Option<f32>,
}
/// What is heavy now: the processes running one program in one folder,
/// counted together (twelve `rustc` of one build are one line).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Heavy {
    /// The program, as the kernel names it.
    pub name: String,
    /// The folder it runs in, where it can be read.
    pub cwd: Option<String>,
    /// Its processes.
    pub pids: Vec<u32>,
    /// Its share of the whole machine's CPU since the reading before, in
    /// percent; unknown on the first reading.
    pub cpu: Option<f32>,
    /// Resident memory, bytes.
    pub memory: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Reading {
    pub at: u64,
    pub cpu: Option<f32>,
    pub memory: Option<Usage>,
    #[serde(default)]
    pub swap: Option<Usage>,
    #[serde(default)]
    pub swap_activity_bps: Option<f32>,
    #[serde(default)]
    pub gpu_reason: Option<String>,
    #[serde(default)]
    pub disks: Vec<Disk>,
    #[serde(default)]
    pub gpus: Vec<Gpu>,
    pub uptime: Option<u64>,
    /// What is heaviest now: read only while someone looks at the host.
    #[serde(default)]
    pub heavy: Vec<Heavy>,
}
/// One minute of the host: the busy figures (CPU, GPU) as the mean of its
/// readings, the full ones (memory, GPU memory, swap, disks) as their
/// highest, since a full minute is what runs out. Percent, one decimal.
/// Encoded as `[at, cpu, memory, gpu, vram, swap, [disks…]]`, the disks in
/// the order of the reading's.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(from = "PointTuple", into = "PointTuple")]
pub struct Point {
    pub at: u64,
    pub cpu: Option<f32>,
    pub memory: Option<f32>,
    pub gpu: Option<f32>,
    pub vram: Option<f32>,
    pub swap: Option<f32>,
    pub disks: Vec<Option<f32>>,
}
type PointTuple = (
    u64,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Vec<Option<f32>>,
);
impl From<PointTuple> for Point {
    fn from((at, cpu, memory, gpu, vram, swap, disks): PointTuple) -> Self {
        Self {
            at,
            cpu,
            memory,
            gpu,
            vram,
            swap,
            disks,
        }
    }
}
impl From<Point> for PointTuple {
    fn from(p: Point) -> Self {
        (p.at, p.cpu, p.memory, p.gpu, p.vram, p.swap, p.disks)
    }
}
/// A percent to one decimal, as points carry it.
pub fn tenth(v: f32) -> f32 {
    (v * 10.).round() / 10.
}
impl Point {
    /// The point of a reading.
    pub fn of(r: &Reading) -> Point {
        let gpu = r.gpus.first();
        let pct = |u: Option<Usage>| u.and_then(Usage::percent).map(tenth);
        Point {
            at: r.at,
            cpu: r.cpu.map(tenth),
            memory: pct(r.memory),
            gpu: gpu.and_then(|g| g.util).map(tenth),
            vram: pct(gpu.and_then(|g| g.memory)),
            swap: pct(r.swap.filter(|s| s.total > 0)),
            disks: r.disks.iter().map(|d| pct(Some(d.usage()))).collect(),
        }
    }
}
/// The readings of one minute so far, made into its [`Point`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Minute {
    pub start: u64,
    cpu: Mean,
    gpu: Mean,
    memory: Option<f32>,
    vram: Option<f32>,
    swap: Option<f32>,
    disks: Vec<Option<f32>>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Mean {
    sum: f32,
    n: u32,
}
impl Mean {
    fn add(&mut self, v: Option<f32>) {
        if let Some(v) = v {
            self.sum += v;
            self.n += 1;
        }
    }
    fn get(self) -> Option<f32> {
        (self.n > 0).then(|| tenth(self.sum / self.n as f32))
    }
}
fn most(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}
/// How long a [`Minute`] is, in ms.
pub const MINUTE: u64 = 60_000;
impl Minute {
    pub fn starting(at: u64) -> Minute {
        Minute {
            start: at - at % MINUTE,
            ..Minute::default()
        }
    }
    /// Whether `at` falls in this minute.
    pub fn holds(&self, at: u64) -> bool {
        at - at % MINUTE == self.start
    }
    pub fn add(&mut self, p: &Point) {
        self.cpu.add(p.cpu);
        self.gpu.add(p.gpu);
        self.memory = most(self.memory, p.memory);
        self.vram = most(self.vram, p.vram);
        self.swap = most(self.swap, p.swap);
        if self.disks.len() < p.disks.len() {
            self.disks.resize(p.disks.len(), None);
        }
        for (mine, theirs) in self.disks.iter_mut().zip(&p.disks) {
            *mine = most(*mine, *theirs);
        }
    }
    pub fn point(&self) -> Point {
        Point {
            at: self.start,
            cpu: self.cpu.get(),
            memory: self.memory,
            gpu: self.gpu.get(),
            vram: self.vram,
            swap: self.swap,
            disks: self.disks.clone(),
        }
    }
}
/// What the host channel opens with: the machine, its day in minutes
/// (oldest first, the minute still filling last), its reading now and what
/// happened to the node.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub facts: Facts,
    pub minutes: Vec<Point>,
    pub latest: Option<Reading>,
    #[serde(default)]
    pub events: Vec<Event>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReadingFrame {
    pub reading: Reading,
    pub point: Point,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EventFrame {
    pub event: Event,
}
/// One unit or probe of a service, as systemd says it is.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Component {
    /// The unit's name, or the probe's URL.
    pub unit: String,
    /// `active`, `inactive`, `failed`, `activating`, `deactivating`,
    /// `reloading` or `unknown`.
    pub state: String,
    /// systemd's finer word (`running`, `dead`, `auto-restart`), if any.
    #[serde(default)]
    pub sub: String,
    /// When it entered its state, Unix ms.
    #[serde(default)]
    pub since: Option<u64>,
    /// How often systemd has restarted it on its own.
    #[serde(default)]
    pub restarts: u32,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Service {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub link: String,
    /// `running`, `stopped`, `failed`, `partial`, `starting`, `stopping`
    /// or `unknown`.
    pub state: String,
    pub controllable: bool,
    #[serde(default)]
    pub components: Vec<Component>,
}
impl Service {
    /// When the service entered its state: the latest of its units'.
    pub fn since(&self) -> Option<u64> {
        self.components.iter().filter_map(|c| c.since).max()
    }
    /// How often systemd restarted its units on its own.
    pub fn restarts(&self) -> u32 {
        self.components.iter().map(|c| c.restarts).sum()
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Done,
    Failed,
    Unconfirmed,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceList {
    pub services: Vec<Service>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceOne {
    pub service: Service,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceAction {
    pub service: Service,
    pub errors: Vec<String>,
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<parvi_core::Error>,
}
/// One line of a service's log.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogLine {
    /// Unix ms.
    pub at: u64,
    pub text: String,
    /// Whether the unit logged it as an error or worse.
    #[serde(default)]
    pub error: bool,
}
/// A service's latest log lines, oldest first, and why some could not be
/// read.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceLog {
    pub lines: Vec<LogLine>,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minute_means_the_busy_figures_and_keeps_the_fullest() {
        let mut m = Minute::starting(MINUTE + 5);
        assert!(m.holds(MINUTE * 2 - 1) && !m.holds(MINUTE * 2));
        let p = |cpu: f32, memory: f32, disk: Option<f32>| Point {
            at: MINUTE + 1,
            cpu: Some(cpu),
            memory: Some(memory),
            disks: vec![Some(50.), disk],
            ..Point::default()
        };
        m.add(&p(10., 40., None));
        m.add(&p(30., 90., Some(7.)));
        m.add(&p(20., 60., None));
        let point = m.point();
        assert_eq!(point.at, MINUTE);
        assert_eq!(point.cpu, Some(20.));
        assert_eq!(point.memory, Some(90.), "the fullest moment, not the mean");
        assert_eq!(point.disks, vec![Some(50.), Some(7.)]);
        assert_eq!(point.gpu, None, "nothing read stays unknown");
    }

    #[test]
    fn a_point_travels_as_a_short_tuple() {
        let p = Point {
            at: 7,
            cpu: Some(12.5),
            disks: vec![Some(60.), None],
            ..Point::default()
        };
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "[7,12.5,null,null,null,null,[60.0,null]]");
        assert_eq!(serde_json::from_str::<Point>(&json).unwrap(), p);
    }

    #[test]
    fn the_point_of_a_reading_rounds_and_leaves_no_swap_unknown() {
        let r = Reading {
            at: 3,
            cpu: Some(12.345),
            memory: Some(Usage { used: 1, total: 3 }),
            swap: Some(Usage { used: 0, total: 0 }),
            disks: vec![Disk {
                name: "/".into(),
                mount: "/".into(),
                used: 1,
                total: 4,
            }],
            ..Reading::default()
        };
        let p = Point::of(&r);
        assert_eq!(p.cpu, Some(12.3));
        assert_eq!(p.memory, Some(33.3));
        assert_eq!(p.swap, None);
        assert_eq!(p.disks, vec![Some(25.)]);
    }
}
