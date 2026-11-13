//! The host channel's frames: the app turns the stream on or off, and the
//! node answers with the whole state, then each reading as it is taken and
//! each event as it happens. Bodies are the contract's types as JSON.

use anyhow::{Result, bail};

use parvi_host_contract::{
    Event as WireEvent, EventFrame, Facts, Point, Reading, ReadingFrame, State,
};
use parvi_node_kit::events::Event;

pub use parvi_host_contract::{HOST, HOST_EVENT, HOST_READING, HOST_STATE, encode_on};

/// Whether a client frame belongs to this channel.
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

fn framed(kind: u8, body: &impl serde::Serialize) -> Vec<u8> {
    let mut frame = vec![kind];
    serde_json::to_writer(&mut frame, body).expect("host frames are JSON");
    frame
}

fn wire(e: &Event) -> WireEvent {
    WireEvent {
        at: e.at,
        kind: e.kind.to_owned(),
        failure: e.failure,
        text: e.text.clone(),
    }
}

/// The state frame.
pub fn state(
    facts: &Facts,
    minutes: Vec<Point>,
    latest: Option<&Reading>,
    events: &[Event],
) -> Vec<u8> {
    framed(
        HOST_STATE,
        &State {
            facts: facts.clone(),
            minutes,
            latest: latest.cloned(),
            events: events.iter().map(wire).collect(),
        },
    )
}

pub fn event(event: &Event) -> Vec<u8> {
    framed(HOST_EVENT, &EventFrame { event: wire(event) })
}

/// A reading and the point it makes.
pub fn reading(reading: &Reading) -> Vec<u8> {
    framed(
        HOST_READING,
        &ReadingFrame {
            point: Point::of(reading),
            reading: reading.clone(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_and_off_round_trip() {
        assert!(owns(&encode_on(true)));
        assert!(decode_on(&encode_on(true)).unwrap());
        assert!(!decode_on(&encode_on(false)).unwrap());
        assert!(decode_on(&[HOST]).is_err());
        assert!(!owns(&[0x10, 0, 0, 0, 0]));
    }

    #[test]
    fn frames_carry_the_contracts_types() {
        let r = Reading {
            at: 1000,
            cpu: Some(14.25),
            ..Reading::default()
        };
        let frame = reading(&r);
        assert_eq!(frame[0], HOST_READING);
        let parsed: ReadingFrame = serde_json::from_slice(&frame[1..]).unwrap();
        assert_eq!(parsed.reading, r);
        assert_eq!(parsed.point.cpu, Some(14.3));

        let events = [Event {
            at: 7,
            kind: "peer",
            failure: true,
            text: "rig went \"away\"".into(),
        }];
        let frame = state(&Facts::default(), vec![Point::of(&r)], Some(&r), &events);
        assert_eq!(frame[0], HOST_STATE);
        let parsed: State = serde_json::from_slice(&frame[1..]).unwrap();
        assert_eq!(parsed.minutes.len(), 1);
        assert_eq!(parsed.events[0].text, "rig went \"away\"");
        assert_eq!(parsed.latest, Some(r));
    }
}
