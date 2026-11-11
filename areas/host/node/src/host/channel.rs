//! The host channel's frames, see `docs/protocol.md`: the browser turns the
//! stream on or off, and the server answers with the whole state, then each
//! reading as it is taken.

use std::fmt::Write;

use anyhow::{Result, bail};

use super::disk;
use super::facts::Facts;
use super::history::{BUCKET, History};
use super::reading::{Point, Reading, Usage};
use parvi_host_contract::{Disk, Reading as WireReading};
use parvi_node_kit::events::Event;
fn push_json_string(out: &mut String, value: &str) {
    out.push_str(&serde_json::to_string(value).expect("string JSON"));
}

pub use parvi_host_contract::{HOST, HOST_EVENT, HOST_READING, HOST_STATE, encode_on};

/// Whether a browser frame belongs to this channel.
pub fn owns(frame: &[u8]) -> bool {
    frame.first() == Some(&HOST)
}

/// The on or off of a host frame.
pub fn decode_on(frame: &[u8]) -> Result<bool> {
    match frame {
        [HOST, on] => Ok(*on != 0),
        _ => bail!("host frame needs one byte"),
    }
}

pub struct State<'a> {
    pub facts: &'a Facts,
    pub every_ms: u64,
    pub history: &'a History,
    pub latest: Option<&'a Reading>,
    pub events: &'a [Event],
}

pub fn state(state: &State) -> Vec<u8> {
    let mut json = String::from("{\"facts\":");
    push_facts(&mut json, state.facts);
    let _ = write!(
        json,
        ",\"every\":{},\"bucket\":{BUCKET},\"earlier\":",
        state.every_ms
    );
    push_points(&mut json, state.history.earlier());
    json.push_str(",\"history\":");
    push_points(&mut json, state.history.recent());
    json.push_str(",\"latest\":");
    match state.latest {
        Some(reading) => push_reading(&mut json, &wire_reading(reading)),
        None => json.push_str("null"),
    }
    json.push_str(",\"events\":[");
    for (i, e) in state.events.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        push_event(&mut json, e);
    }
    json.push_str("]}");
    [&[HOST_STATE][..], json.as_bytes()].concat()
}

pub fn event(event: &Event) -> Vec<u8> {
    let mut json = String::from("{\"event\":");
    push_event(&mut json, event);
    json.push('}');
    [&[HOST_EVENT][..], json.as_bytes()].concat()
}

/// `{"at":ms,"kind":"peer","failure":false,"text":"…"}`.
fn push_event(json: &mut String, e: &Event) {
    let _ = write!(json, "{{\"at\":{},\"kind\":", e.at);
    push_json_string(json, e.kind);
    let _ = write!(json, ",\"failure\":{},\"text\":", e.failure);
    push_json_string(json, &e.text);
    json.push('}');
}

/// A reading and the history point it makes.
pub fn reading(reading: &Reading, point: &Point) -> Vec<u8> {
    let mut json = String::from("{\"reading\":");
    push_reading(&mut json, &wire_reading(reading));
    json.push_str(",\"point\":");
    push_point(&mut json, point);
    json.push('}');
    [&[HOST_READING][..], json.as_bytes()].concat()
}

fn push_facts(json: &mut String, facts: &Facts) {
    json.push('{');
    for (i, (key, value)) in [
        ("os", &facts.os),
        ("kernel", &facts.kernel),
        ("cpu", &facts.cpu),
        ("user", &facts.user),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            json.push(',');
        }
        let _ = write!(json, "\"{key}\":");
        push_text(json, value.as_deref());
    }
    let _ = write!(json, ",\"threads\":{}}}", facts.threads);
}

fn push_points<'a>(json: &mut String, points: impl Iterator<Item = &'a Point>) {
    json.push('[');
    for (i, point) in points.enumerate() {
        if i > 0 {
            json.push(',');
        }
        push_point(json, point);
    }
    json.push(']');
}

/// `[at, cpu, memory, gpu, load, swap, vram]`, unknowns as null.
fn push_point(json: &mut String, point: &Point) {
    let _ = write!(json, "[{}", point.at);
    for figure in [
        point.cpu,
        point.memory,
        point.gpu,
        point.load,
        point.swap,
        point.vram,
    ] {
        json.push(',');
        push_number(json, figure);
    }
    json.push(']');
}

fn wire_reading(reading: &Reading) -> WireReading {
    WireReading {
        at: reading.at,
        cpu: reading.cpu,
        memory: reading.memory,
        swap: reading.swap,
        load: reading.load.map(|load| [load.one, load.five, load.fifteen]),
        disk: reading.disk.map(|usage| Disk {
            mount: disk::mount(),
            used: usage.used,
            total: usage.total,
        }),
        gpus: reading.gpus.clone(),
        uptime: reading.uptime,
    }
}

fn push_reading(json: &mut String, reading: &WireReading) {
    let _ = write!(json, "{{\"at\":{},\"cpu\":", reading.at);
    push_number(json, reading.cpu);
    json.push_str(",\"memory\":");
    push_usage(json, reading.memory);
    json.push_str(",\"swap\":");
    push_usage(json, reading.swap);
    json.push_str(",\"load\":");
    match reading.load {
        Some([one, five, fifteen]) => {
            let _ = write!(json, "[{one:.2},{five:.2},{fifteen:.2}]");
        }
        None => json.push_str("null"),
    }
    json.push_str(",\"disk\":");
    match &reading.disk {
        Some(usage) => {
            json.push_str("{\"mount\":");
            push_json_string(json, &usage.mount);
            let _ = write!(json, ",\"used\":{},\"total\":{}}}", usage.used, usage.total);
        }
        None => json.push_str("null"),
    }
    json.push_str(",\"gpus\":[");
    for (i, gpu) in reading.gpus.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str("{\"name\":");
        push_json_string(json, &gpu.name);
        json.push_str(",\"util\":");
        push_number(json, gpu.util);
        json.push_str(",\"memory\":");
        push_usage(json, gpu.memory);
        json.push_str(",\"temp\":");
        push_number(json, gpu.temp);
        json.push('}');
    }
    json.push_str("],\"uptime\":");
    match reading.uptime {
        Some(seconds) => {
            let _ = write!(json, "{seconds}");
        }
        None => json.push_str("null"),
    }
    json.push('}');
}

fn push_usage(json: &mut String, usage: Option<Usage>) {
    match usage {
        Some(Usage { used, total }) => {
            let _ = write!(json, "{{\"used\":{used},\"total\":{total}}}");
        }
        None => json.push_str("null"),
    }
}

/// One decimal; null when unknown or not a number.
fn push_number(json: &mut String, value: Option<f32>) {
    match value.filter(|v| v.is_finite()) {
        Some(value) => {
            let _ = write!(json, "{value:.1}");
        }
        None => json.push_str("null"),
    }
}

fn push_text(json: &mut String, value: Option<&str>) {
    match value {
        Some(text) => push_json_string(json, text),
        None => json.push_str("null"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::reading::{Gpu, Load};

    fn facts() -> Facts {
        Facts {
            os: Some("Ubuntu \"24.04\"".into()),
            kernel: None,
            cpu: Some("Ryzen".into()),
            threads: 32,
            user: Some("operator".into()),
        }
    }

    fn sample() -> Reading {
        Reading {
            at: 1000,
            cpu: Some(14.25),
            memory: Some(Usage { used: 1, total: 4 }),
            swap: Some(Usage { used: 1, total: 8 }),
            load: Some(Load {
                one: 0.42,
                five: 0.731,
                fifteen: 0.6,
            }),
            disk: Some(Usage { used: 2, total: 8 }),
            gpus: vec![Gpu {
                name: "RTX 4090".into(),
                util: Some(87.0),
                memory: None,
                temp: Some(61.0),
            }],
            uptime: Some(3600),
        }
    }

    fn json(frame: &[u8]) -> &str {
        std::str::from_utf8(&frame[1..]).unwrap()
    }

    #[test]
    fn on_and_off_round_trip() {
        assert!(owns(&encode_on(true)));
        assert!(decode_on(&encode_on(true)).unwrap());
        assert!(!decode_on(&encode_on(false)).unwrap());
        assert!(decode_on(&[HOST]).is_err());
        assert!(!owns(&[0x10, 0, 0, 0, 0]));
    }

    #[test]
    fn a_reading_carries_its_point() {
        let sample = sample();
        let frame = reading(&sample, &crate::host::reading::point_of(&sample, 4));
        assert_eq!(frame[0], HOST_READING);
        let parsed: parvi_host_contract::ReadingFrame =
            serde_json::from_slice(&frame[1..]).unwrap();
        assert_eq!(parsed.reading.at, sample.at);
        assert_eq!(parsed.point.at, sample.at);
        // The mount is `/`, or `C:\` on Windows.
        let mount = serde_json::to_string(&disk::mount()).unwrap();
        assert_eq!(
            json(&frame),
            format!(
                "{{\"reading\":{{\"at\":1000,\"cpu\":14.2,\"memory\":{{\"used\":1,\"total\":4}},\
                 \"swap\":{{\"used\":1,\"total\":8}},\"load\":[0.42,0.73,0.60],\
                 \"disk\":{{\"mount\":{mount},\"used\":2,\"total\":8}},\
                 \"gpus\":[{{\"name\":\"RTX 4090\",\"util\":87.0,\"memory\":null,\"temp\":61.0}}],\
                 \"uptime\":3600}},\"point\":[1000,14.2,25.0,87.0,10.5,12.5,null]}}"
            )
        );
    }

    #[test]
    fn the_state_holds_facts_history_oldest_first_and_the_latest() {
        let mut history = History::new(2, 4);
        for at in [1, 2, BUCKET + 1, BUCKET + 2, BUCKET + 3] {
            history.push(Point {
                at,
                memory: Some(50.0),
                ..Point::default()
            });
        }
        let frame = state(&State {
            facts: &facts(),
            every_ms: 5000,
            history: &history,
            latest: None,
            events: &[Event {
                at: 7,
                kind: "peer",
                failure: true,
                text: "rig went \"away\"".into(),
            }],
        });
        assert_eq!(frame[0], HOST_STATE);
        let parsed: parvi_host_contract::State = serde_json::from_slice(&frame[1..]).unwrap();
        assert_eq!(parsed.facts, facts());
        assert_eq!(parsed.history.len(), 2);
        assert_eq!(parsed.events[0].text, "rig went \"away\"");
        assert_eq!(
            json(&frame),
            "{\"facts\":{\"os\":\"Ubuntu \\\"24.04\\\"\",\"kernel\":null,\"cpu\":\"Ryzen\",\"user\":\"operator\",\"threads\":32},\
             \"every\":5000,\"bucket\":60000,\"earlier\":[[0,null,50.0,null,null,null,null]],\
             \"history\":[[60002,null,50.0,null,null,null,null],[60003,null,50.0,null,null,null,null]],\
             \"latest\":null,\
             \"events\":[{\"at\":7,\"kind\":\"peer\",\"failure\":true,\"text\":\"rig went \\\"away\\\"\"}]}"
        );
    }
}
