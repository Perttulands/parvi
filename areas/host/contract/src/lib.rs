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
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Disk {
    #[serde(default)]
    pub mount: String,
    pub used: u64,
    pub total: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub util: Option<f32>,
    pub memory: Option<Usage>,
    pub temp: Option<f32>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Reading {
    pub at: u64,
    pub cpu: Option<f32>,
    pub memory: Option<Usage>,
    #[serde(default)]
    pub swap: Option<Usage>,
    #[serde(default)]
    pub load: Option<[f32; 3]>,
    pub disk: Option<Disk>,
    #[serde(default)]
    pub gpus: Vec<Gpu>,
    pub uptime: Option<u64>,
}
/// History is encoded as `[at, cpu, memory, gpu, load, swap, vram]`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(from = "PointTuple", into = "PointTuple")]
pub struct Point {
    pub at: u64,
    pub cpu: Option<f32>,
    pub memory: Option<f32>,
    pub gpu: Option<f32>,
    pub load: Option<f32>,
    pub swap: Option<f32>,
    pub vram: Option<f32>,
}
type PointTuple = (
    u64,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
);
impl From<PointTuple> for Point {
    fn from((at, cpu, memory, gpu, load, swap, vram): PointTuple) -> Self {
        Self {
            at,
            cpu,
            memory,
            gpu,
            load,
            swap,
            vram,
        }
    }
}
impl From<Point> for PointTuple {
    fn from(p: Point) -> Self {
        (p.at, p.cpu, p.memory, p.gpu, p.load, p.swap, p.vram)
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub facts: Facts,
    pub every: u64,
    pub bucket: u64,
    pub earlier: Vec<Point>,
    pub history: Vec<Point>,
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
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Service {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub link: String,
    pub state: String,
    pub controllable: bool,
    #[serde(default)]
    pub components: Vec<String>,
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
